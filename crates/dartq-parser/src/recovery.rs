//! Recovery primitives.
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
//!   token for an unclosed group (recording the unmatched pair on the token
//!   table instead of reporting), adopting it REPORTS the row at the
//!   relocation point (the official translation reads
//!   `begin.endGroup.charOffset` post-parse) and the parse-end
//!   [`Self::report_unmatched_groups`] pass reports the never-adopted pairs
//!   at their parked positions;
//! * skip-to-sync — group-aware jump to a boundary set, with a no-progress
//!   guard at every loop level (official strategy C, `parser_impl.dart:445`).

use dartq_scanner::{Keyword, TokenKind};

use crate::listener::IdentifierContext;
use crate::members::is_keyword_or_identifier;
use crate::message::msg;
use crate::parser::{AsyncModifier, Parser};
use crate::token_id::TokenSpan;

/// Backward-scan budget of [`Parser::locate_spill_mark`]: the stranded run
/// of a primary bracket error begins at the mismatching token, so the
/// distance from a spill head back to the parked synthetic is the run plus
/// at most the statement parse that followed it — well under this bound;
/// heads beyond it keep the official lone-junk stall shape.
const SPILL_SCAN_LIMIT: usize = 64;

impl<L: crate::listener::ParserListener> Parser<'_, '_, '_, L> {
    /// Reports a recoverable error pinned to `at` (official
    /// `reportRecoverableError`, `parser_impl.dart:11399`).
    ///
    /// Identical `(offset, length, code)` rows collapse to one
    /// ([`Self::admit_row`]). Counted against the per-file diagnostics cap:
    /// suppressed once the bailout latched; a deduped duplicate burns no
    /// capacity. The official zero-width-synthetic anchor walk
    /// (`findNonZeroLengthToken`, parser/util.dart:56) is applied at the call
    /// sites that can anchor on a parser-inserted synthetic (see
    /// `parse_constructor_invocation_arguments`); every other site anchors on
    /// a real token by construction.
    pub(crate) fn report(&mut self, message: crate::message::Message, at: TokenSpan) {
        if self.admit_row(at.range.offset, at.range.length, &message) && self.admit_diagnostic() {
            self.listener.handle_recoverable_error(message, at, at);
        }
    }

    /// Whether this row is the first of its `(offset, length, code, text)`
    /// quadruple. The analyzer dedupes same-offset same-code errors with the
    /// same rendered text, so a recovery cascade re-reporting a row at an
    /// already-reported anchor surfaces once (oracle `const_patterns` 46:24
    /// `expected_token ';'` and `const_patterns_binary` 85:12: two recovery
    /// points, one row; the differently-coded `missing_identifier` at 85:12
    /// survives). The text is part of the key: at the fasta layer the deduped
    /// reports are distinct diagnostics (`expectedButGot` vs `expectedToken`)
    /// sharing one registry code, and the oracle keeps both when their texts
    /// differ (76dd104c 12:23 surfaces `Expected to find ','.` AND
    /// `Expected to find ';'.` over one span).
    fn admit_row(&mut self, offset: u32, length: u32, message: &crate::message::Message) -> bool {
        // Post-bail rows are suppressed by `admit_diagnostic` anyway (which
        // is also why the `&&` operand order in [`Self::report`] must not be
        // swapped: an early `admit_diagnostic` would burn cap capacity — and
        // could trip the fuse — on rows the dedup would have dropped).
        // Skipping the dedup work here is behavior-identical: `bailed`
        // short-circuits `admit_diagnostic` with no side effects.
        if self.is_bailed() {
            return false;
        }
        // Hash pre-filter keyed on `(offset, length, code)`: a triple never
        // seen is definitionally a new row (`insert` reports fresh keys as
        // `true`), so the common case is one O(1) hash insert and the exact
        // four-field scan (O(k) expected per report versus the previous
        // O(k²) per-cascade linear scan) only runs on triple collisions —
        // distinct-text rows over one span are rare (76dd104c 12:23 is the
        // pinned face).
        if !self.row_keys.insert((offset, length, message.code.name))
            && self.reported_rows.iter().any(|(o, l, c, t)| {
                *o == offset && *l == length && *c == message.code.name && *t == message.text
            })
        {
            return false;
        }
        self.reported_rows
            .push((offset, length, message.code.name, message.text.clone()));
        true
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

    /// `expectedButGot` flavor; the analyzer converter re-renders every
    /// `PseudoSharedCode.expectedToken` message with the registry template,
    /// so the rendered text matches [`Self::expect_kind`]'s row (see
    /// `msg::expected_but_got`).
    pub(crate) fn expect_lexeme_but_got(&mut self, expected: &str) -> TokenSpan {
        let at = self.peek_span();
        self.expect_lexeme_but_got_anchored(expected, at)
    }

    /// [`Self::expect_lexeme_but_got`] with an explicit report anchor: the
    /// synthetic still splices at the current token, but the row anchors on
    /// `anchor` — the #2024 separator recovery anchor (the token the
    /// recovery lands behind, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart
    /// `rewriteAndRecover` reports at the resumed token).
    pub(crate) fn expect_lexeme_but_got_anchored(
        &mut self,
        expected: &str,
        anchor: TokenSpan,
    ) -> TokenSpan {
        if let Some(id) = self.cursor.opt_lexeme(expected) {
            return self.span_of(id);
        }
        let at = self.peek_span();
        self.report(msg::expected_but_got(expected), anchor);
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
        // identifier_context_impl.dart:200/391) — the gate is the
        // continuation property itself, so every continuation context
        // takes it. Cascade sections consume their `..new` in
        // `parse_send_continuation` before any identifier entry, so the
        // set here covers the dot-continuation paths that do reach this
        // gate (selector chains, dot shorthands, pattern selectors).
        if peek.kind == TokenKind::Keyword(Keyword::New)
            && matches!(
                context,
                crate::listener::IdentifierContext::ConstructorReferenceContinuation
                    | crate::listener::IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments
                    | crate::listener::IdentifierContext::ExpressionContinuation
                    | crate::listener::IdentifierContext::MethodDeclarationContinuation
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
        // `LibraryIdentifierContext.ensureIdentifier` recovery branch 2b
        // (identifier_context_impl.dart:977-991) and
        // `TopLevelDeclarationIdentifierContext.ensureIdentifier` recovery
        // branch 1 (:1321-1332): a reserved word that heads a top-level
        // declaration is never eaten as the library name / variable name —
        // report `missing_identifier` at it, splice the synthetic name, and
        // leave the keyword current for the declaration loop (oracle
        // `library` + `void f()` and `var class VarClass {}`). The head is
        // only a declaration head when the FOLLOWER does not continue the
        // name — the official templates accept the keyword as the name when
        // the next token is one of the context's continuation values
        // (`_isOneOfFollowingValues`: `.` `;` EOF for the library name;
        // `followingValues` `=` `,` `;` for the variable name), so
        // `library enum;` and `int class = 42;` take the
        // `expected_identifier_but_got_keyword` face (oracle
        // issue_48371_prime1 / keyword_named_top_level_fields line 9).
        if matches!(
            peek.kind,
            TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Reserved
        ) && matches!(
            context,
            crate::listener::IdentifierContext::LibraryName
                | crate::listener::IdentifierContext::TopLevelVariableDeclaration
        ) && self.looks_like_start_of_next_top_level_declaration_at(0)
            && !self.name_continuation_follower(context)
        {
            self.report(msg::missing_identifier(), peek.span);
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
            self.cursor.advance();
            // Official `ensureIdentifier` fires `handleIdentifier` on the
            // recovery template's returned identifier (parser_impl.dart:
            // 4083) — the spliced nameless declaration's event, so the
            // declaration joins the elements under the empty name.
            let span = self.span_of(id);
            self.listener.handle_identifier(span, context);
            return span;
        }
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
            // ExpressionIdentifierContext.ensureIdentifier
            // (identifier_context_impl.dart:397-406): `await` followed by
            // another identifier does not form a valid expression-name —
            // report `unexpectedToken` on the `await` token itself and take
            // the next token as the name (skipping the `await`).
            if matches!(
                context,
                crate::listener::IdentifierContext::Expression
                    | crate::listener::IdentifierContext::ExpressionContinuation
            ) && peek.kind == TokenKind::Keyword(Keyword::Await)
                && is_identifier(self.cursor.look_kind(1))
            {
                self.report(
                    msg::unexpected_token(self.cursor.lexeme(peek.span.id)),
                    peek.span,
                );
                self.cursor.advance(); // `await`
                let id = self.cursor.advance(); // the name token
                let span = self.span_of(id);
                self.listener.handle_identifier(span, context);
                return span;
            }
            // Analyzer field/variable name check: a built-in keyword that
            // names a top-level declaration (`mixin`, `extension`, ...) is a
            // legal variable name only while the next token continues the
            // declaration (`=`, `,`, `;`); otherwise it reads as the next
            // declaration's head — report `missing_identifier`, splice the
            // synthetic name, and leave the keyword for the declaration loop
            // (oracle: `augment mixin M {}` reports at `mixin` and reparses
            // the mixin; `int mixin = 3;` stays clean). Non-declaration
            // built-ins (`abstract`, `covariant`, ...) stay accepted as
            // names like any identifier (`augment abstract class C {}`
            // keeps `abstract` as the field name).
            if matches!(
                context,
                crate::listener::IdentifierContext::FieldDeclaration
                    | crate::listener::IdentifierContext::TopLevelVariableDeclaration
            ) && matches!(
                // Only non-reserved (built-in) heads reach here — `class`/
                // `enum` are reserved and never pass `is_identifier`.
                peek.kind,
                TokenKind::Keyword(
                    Keyword::Augment
                        | Keyword::Extension
                        | Keyword::Export
                        | Keyword::Import
                        | Keyword::Library
                        | Keyword::Mixin
                        | Keyword::Part
                        | Keyword::Typedef
                )
            ) && !matches!(
                self.cursor.look_kind(1),
                TokenKind::Eq | TokenKind::Comma | TokenKind::Semicolon
            ) {
                self.report(msg::missing_identifier(), peek.span);
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
                self.cursor.advance();
                return self.span_of(id);
            }
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
        // Official recovery template, three branches
        // (identifier_context_impl.dart:25-45): follower keep-sets route to
        // insert-WITHOUT-consuming (branch 2 — the offending token belongs
        // to the caller's grammar); other junk takes the consume-in-doubt
        // branch (branch 3, "when in doubt, consume the token", :39-42).
        // Identifier-shaped heads never reach here.
        //
        // The keep set joins the official `_isOneOfFollowingValues` grammar
        // followers (:572-584, formalParameterDeclaration: `:` `=` `,` `(`
        // `)` `[` `]` `{` `}` EOF) with `;`/`>` (the same follower class)
        // and the generic statement-start probe: kept tokens leave the
        // junk current, the synthetic riding in front of it. The consume
        // posture is PER-CONTEXT (#564 r4): only the templates whose
        // official else-branch consumes true junk take it —
        // formalParameterDeclaration (:604-611, oracle `void f(5) {}`: one
        // `missing_identifier` at the `5`, junk consumed) and the
        // library/part name template (:991-1000, oracle `library 5;`: one
        // `missing_identifier` at the `5`); every other context keeps the
        // leave posture so the caller's cascade stands (oracle
        // `class C { new = 42; }`: two rows at the `42`).
        self.report(msg::missing_identifier(), peek.span);
        let leave_for_grammar = matches!(
            peek.kind,
            TokenKind::Comma
                | TokenKind::CloseParen
                | TokenKind::Colon
                | TokenKind::Semicolon
                | TokenKind::Eq
                | TokenKind::Gt
                | TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::CloseSquare
                | TokenKind::OpenCurly
                | TokenKind::CloseCurly
                | TokenKind::Eof
        ) || self.looks_like_statement_start_at(0);
        // Branch 3 for the variable-name contexts
        // (TopLevelDeclarationIdentifierContext:1338-1345 and
        // FieldDeclarationIdentifierContext:493-501): non-keep-set junk is
        // consumed in doubt and the synthetic splices in AFTER it
        // (`rewriter.insertSyntheticIdentifier(identifier)` — the junk lands
        // behind the parse pointer), so a following ensureSemicolon's
        // findPreviousNonZeroLengthToken walk-back anchors on the junk token
        // itself (oracle `void<int> x, ?y;`: the `?` is absorbed by the
        // member and `expected_token ';'` anchors at the `?`).
        if !leave_for_grammar
            && matches!(
                context,
                crate::listener::IdentifierContext::TopLevelVariableDeclaration
                    | crate::listener::IdentifierContext::FieldDeclaration
            )
        {
            let junk_end = peek.span.range.offset + peek.span.range.length;
            self.cursor.advance();
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, junk_end);
            return self.span_of(id);
        }
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
        if leave_for_grammar {
            // Branch 2: the official `insertSyntheticIdentifier(token)`
            // (token_stream_rewriter.dart:238) inserts after the anchor and
            // RETURNS the synthetic, so the official parse resumes with the
            // synthetic as the current token and the offending follower as
            // `token.next`. Drain to the queued synthetic (the
            // ensureSemicolon idiom) for the same posture — an undrained
            // queue resurfaces the zero-width identifier as a phantom
            // parse: `void f(int,, int b)` then re-enters the parameter
            // loop's tail recovery and stacks an `expected_token` on the
            // oracle's single missing_identifier.
            self.cursor.drain_to(id);
        } else if matches!(
            context,
            crate::listener::IdentifierContext::FormalParameterDeclaration
                | crate::listener::IdentifierContext::LibraryName
        ) {
            // Branch 3, aligned: the rewriter splices the synthetic after
            // the junk (insertToken, :84), so the junk lands behind the
            // parse pointer. Drain to the synthetic, then consume the junk
            // for the same stream shape.
            self.cursor.drain_to(id);
            self.cursor.advance();
        } else {
            // Leave posture (the pre-#564 branch-3 shape): the junk stays
            // current; the caller's grammar cascades against it.
            self.cursor.advance();
        }
        let span = self.span_of(id);
        // Official `ensureIdentifier` fires `handleIdentifier` on the
        // recovery template's returned synthetic (parser_impl.dart:4083) —
        // the enum-constant constructor selector's empty-name event rides
        // it (official trace, issue_48380_1: `handleIdentifier(,
        // constructorReferenceContinuationAfterTypeArguments)`). Fired for
        // the constructor-reference continuations only — their downstream
        // consumers are name-gated, so every other recovery face stays
        // byte-stable.
        if matches!(
            context,
            crate::listener::IdentifierContext::ConstructorReferenceContinuation
                | crate::listener::IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments
        ) {
            self.listener.handle_identifier(span, context);
        }
        span
    }

    /// Official missing-identifier report span (upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/parser/util.dart
    /// `findNonZeroLengthToken` + the analyzer's `handleRecoverableError`
    /// offset/end formula): a real got token anchors on its own span; a
    /// zero-width got (the scanner's synthetic interpolation closer parked
    /// at the discarded `${`) keeps the got offset and borrows the first
    /// non-zero-length token's length (frozen oracle 76dd104c 11:12: row
    /// (182, length 4) — got at 182, walked-to segment `foo.` length 4).
    /// The message is always the registry text `Expected an identifier.` —
    /// the template carries no lexeme parameter.
    fn report_missing_identifier(&mut self, got: crate::token_id::TokenSpan) {
        if got.range.length > 0 {
            self.report(msg::missing_identifier(), got);
            return;
        }
        let mut n = 0usize;
        let mut length = 0u32;
        while self.cursor.look_kind(n) != TokenKind::Eof {
            let span = self.look_span_at(n);
            if span.range.length > 0 {
                length = span.range.length;
                break;
            }
            n += 1;
        }
        self.report(
            msg::missing_identifier(),
            crate::token_id::TokenSpan::new(
                got.id,
                dartq_source::SourceRange {
                    offset: got.range.offset,
                    length,
                },
            ),
        );
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
                let lexeme = self.cursor.lexeme(peek.span.id);
                if is_continuation
                    || (!matches!(junk_kind, TokenKind::Eof) && lexeme != "as" && lexeme != "is")
                {
                    self.report(msg::expected_identifier_but_got_keyword(lexeme), peek.span);
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
                        // The remaining operator-token flags of the official
                        // `TokenType.isOperator` set (upstream Dart SDK
                        // source, pkg/_fe_analyzer_shared scanner token.dart):
                        // these stay in the stream behind the spliced name so
                        // the precedence loop consumes them — swallowing one
                        // splits the statement and re-anchors the enclosing
                        // semicolon recovery on the operator (#2024, frozen
                        // oracle 0c9fda4b L85: `?? 2` parses as ONE statement
                        // with the `;` row on the `2`).
                        | TokenKind::AmpersandAmpersand
                        | TokenKind::BarBar
                        | TokenKind::MinusMinus
                        | TokenKind::PeriodPeriod
                        | TokenKind::PlusPlus
                        | TokenKind::QuestionQuestion
                        | TokenKind::QuestionQuestionEq
                )
            {
                // When in doubt, consume the token to ensure progress; the
                // report anchors on it and the synthetic lands after.
                self.report_missing_identifier(peek.span);
                self.cursor.advance();
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
                self.cursor.advance();
                return self.span_of(id);
            }
        }
        self.report_missing_identifier(peek.span);
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

    /// `looksLikeStartOfNextTopLevelDeclaration`
    /// (`identifier_context_impl.dart:1650)`: the `isTopLevelKeyword` heads
    /// (token.dart: class/enum/export/extension/import/library/mixin/part/
    /// typedef) plus const/get/final/set/var/void.
    fn looks_like_start_of_next_top_level_declaration_at(&self, n: usize) -> bool {
        matches!(
            self.cursor.look_kind(n),
            TokenKind::Keyword(
                Keyword::Class
                    | Keyword::Const
                    | Keyword::Enum
                    | Keyword::Export
                    | Keyword::Extension
                    | Keyword::Final
                    | Keyword::Get
                    | Keyword::Import
                    | Keyword::Library
                    | Keyword::Mixin
                    | Keyword::Part
                    | Keyword::Set
                    | Keyword::Typedef
                    | Keyword::Var
                    | Keyword::Void
            )
        )
    }

    /// The follower keep-set that lets a declaration-head keyword be eaten
    /// as the name: `LibraryIdentifierContext._isOneOfFollowingValues`
    /// (identifier_context_impl.dart:953-957 — `.` `;` EOF) and the
    /// top-level declaration context's `followingValues` (`=` `,` `;` —
    /// oracle `int class = 42;` names the field, `var class VarClass {}`
    /// reparses the class).
    fn name_continuation_follower(&self, context: crate::listener::IdentifierContext) -> bool {
        let follower = self.cursor.look_kind(1);
        match context {
            crate::listener::IdentifierContext::LibraryName => {
                matches!(
                    follower,
                    TokenKind::Period | TokenKind::Semicolon | TokenKind::Eof
                )
            }
            _ => matches!(
                follower,
                TokenKind::Eq | TokenKind::Comma | TokenKind::Semicolon
            ),
        }
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
    /// `catch_syntax` (not `missing_identifier`) for non-identifier junk,
    /// then routes exactly like the official template — statement starts /
    /// `,` / `)` / EOF keep the token behind a synthetic name (:36-38); other
    /// non-keyword junk does too ("consume in doubt", :39-42 — the rewriter
    /// inserts before the token, keeping it in the stream); a reserved word
    /// itself becomes the name after the report (:44 `return identifier`).
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
        if !self.looks_like_statement_start_at(0)
            && !matches!(
                peek.kind,
                TokenKind::Comma | TokenKind::CloseParen | TokenKind::Eof
            )
            && is_keyword_or_identifier(peek.kind)
        {
            let id = self.cursor.advance();
            let span = self.span_of(id);
            self.listener
                .handle_identifier(span, crate::listener::IdentifierContext::CatchParameter);
            return span;
        }
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, peek.span.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Official `ensureCloseParen` (`parser_impl.dart:5028`): consumes `)`
    /// and returns its span.
    ///
    /// The row text is the shared `EXPECTED_TOKEN` problem text `Expected
    /// to find ')'.`: the 3.13.1 oracle's machine output for the
    /// parameter-group face (`void f(int 5) {}` probe — one row at the
    /// junk); 3.15's source passes `diag.expectedButGot` (:5040), whose
    /// `pseudoSharedCode: PseudoSharedCode.expectedToken`
    /// (diagnostic.g.dart:646-652) surfaces as the same analyzer code, and
    /// the ensureSemicolon adjudication (#459) fixed the same divergence
    /// for `;`.
    ///
    /// Missing `)` with a scanner-synthesized closer for the group: adopt it
    /// (`moveSynthetic`) and report the unmatched-group row at the
    /// relocation point — the official `moveSynthetic` re-offsets the closer
    /// to the insertion point (`_setOffset(endGroup, next.offset)`), and the
    /// analyzer translation reads `begin.endGroup.charOffset` post-parse, so
    /// the row lands exactly here (oracle `new C(;` → `expected_token` ')'
    /// @ the `;`; `f('${{` → ')' @ the `${`). The begin id latches
    /// [`Self::unmatched_reported`] so the parse-end pass skips the pair.
    /// A well-formed but distant `)` is reported once and jumped to.
    /// Adopting the scanner's synthetic marks the spill run of the primary
    /// bracket error (see [`Self::drain_spill_run`]).
    pub(crate) fn ensure_close_paren(&mut self, open: TokenSpan) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(TokenKind::CloseParen) {
            self.mark_spill_run(id);
            return self.span_of(id);
        }
        // Where does the group actually close?
        if let Some(close_id) = self.cursor.table_matching_end(open.id) {
            let (_, synthetic) = self.cursor.resolve(close_id);
            if synthetic {
                // Scanner-synthesized closer parked at the wrong spot (EOF
                // for an unclosed `(`): relocate it (:5035-5037).
                return self.move_synthetic_close(open, close_id, TokenKind::CloseParen, ")");
            }
            let at = self.peek_span();
            self.report(msg::expected_token(")"), at);
            self.skip_to_token(close_id);
            let close = self.peek_span();
            self.cursor.advance();
            return close;
        }
        // No matching closer at all: report and synthesize here.
        let at = self.peek_span();
        self.report(msg::expected_token(")"), at);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::CloseParen, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// Official `moveSynthetic`: relocate the scanner-synthesized closer
    /// `close_id` of `open` in front of the current token and consume it,
    /// reporting the unmatched-group row the post-parse translation emits at
    /// the relocated offset (it reads `begin.endGroup.charOffset`); the
    /// enclosing parse resumes at the current token. The begin id latches
    /// [`Self::unmatched_reported`] so the parse-end pass skips the pair.
    pub(crate) fn move_synthetic_close(
        &mut self,
        open: TokenSpan,
        close_id: crate::token_id::TokenId,
        kind: TokenKind,
        lexeme: &'static str,
    ) -> TokenSpan {
        self.spill_after = Some(self.span_of(close_id).range.offset);
        let at = self.peek_span();
        self.report_unmatched_row(lexeme, at);
        self.unmatched_reported.push(open.id);
        let id = self.cursor.insert_synthetic(kind, at.range.offset);
        self.cursor.advance();
        self.span_of(id)
    }

    /// One unmatched-group row (official `translateErrorToken`'s
    /// `unmatchedToken` arm: `expected_token '<closer>'` at
    /// `begin.endToken.charOffset`, length always 1 —
    /// translate_error_token.dart:89-104 reports
    /// `atOffset(offset: charOffset, length: 1)` (:28), no zero-width shape
    /// exists). At the source end the closer parks at the eof offset
    /// (`appendEofToken` → `unmatchedBeginGroup`, abstract_scanner.dart:529-543,
    /// :901-951) and the eof decrement in `makeError` (:21-27) never fires:
    /// scanner error tokens chain in one run at the stream front
    /// (`prependErrorToken`, abstract_scanner.dart:677-689, seeded by the -1
    /// EOF sentinel), so `_isAtEnd`'s BAD_INPUT-skipping walk (:114-126) hits
    /// the begin token, not EOF — the row is length-1 AT the eof offset
    /// (oracle machine rows: `f('${` → '}' @2:1|1, `void main() {` → '}' at
    /// offset = source length). Identical rows collapse: the oracle dedupes
    /// them downstream (two_unclosed_curly's two `}`-groups parked at EOF
    /// surface as a single row). `at` carries the closer's handle + its
    /// as-scanned span; the encoded range is the translation's law above, not
    /// `at` verbatim.
    fn report_unmatched_row(&mut self, lexeme: &'static str, at: TokenSpan) {
        let eof = self.table.last().utf16_offset;
        let offset = if at.range.offset >= eof {
            eof
        } else {
            at.range.offset
        };
        let key = (lexeme, offset);
        if self.unmatched_seen.iter().any(|&k| k == key) {
            return;
        }
        self.unmatched_seen.push(key);
        self.report(
            msg::expected_token(lexeme),
            TokenSpan::new(at.id, dartq_source::SourceRange { offset, length: 1 }),
        );
    }

    /// The parse-end pass (official `reportAllErrorTokens`,
    /// `parser_impl.dart:11433-11439`): reports every scanner-unmatched
    /// group whose closer was never adopted at a relocation point — the row
    /// sits at the parked closer's position (`begin.endGroup.charOffset`
    /// post-parse; a never-adopted closer keeps its scan-time position: EOF
    /// for `appendEofToken` closers, the mismatching closer for
    /// `insertSyntheticClosers`). Bounded by the pair list; capped through
    /// the shared diagnostic fuse.
    pub(crate) fn report_unmatched_groups(&mut self) {
        let pairs = self.table.unmatched_groups();
        if pairs.is_empty() {
            return;
        }
        for &(begin, close) in pairs {
            if self.unmatched_reported.contains(&begin) {
                continue;
            }
            // Scanner pair indices are stable (original tokens keep their
            // indices through `append_inserted`), so the lookup always
            // succeeds; a miss degrades by skipping the row (under-report
            // only — the parity face is untouched) instead of panicking.
            let closer = match self.table.get(close as usize) {
                Some(t) => t,
                None => {
                    debug_assert!(false, "pair indexes the table");
                    continue;
                }
            };
            let lexeme = closer.kind.fixed_lexeme().unwrap_or("}");
            // The span length is a placeholder — report_unmatched_row encodes
            // the translation's law (length 1; the offset clamps to the eof
            // anchor).
            let at = TokenSpan::new(
                close,
                dartq_source::SourceRange {
                    offset: closer.utf16_offset,
                    length: 0,
                },
            );
            self.report_unmatched_row(lexeme, at);
        }
    }

    /// Skips forward until `id` is the current token (group boundaries in the
    /// table are respected implicitly: ids are absolute). EOF pins the
    /// cursor, so a stale target cannot loop forever.
    pub(crate) fn skip_to_token(&mut self, id: crate::token_id::TokenId) {
        while self.cursor.peek_id() != id && !self.cursor.at_eof() {
            self.cursor.advance();
        }
    }

    /// Marks [`Self::drain_spill_run`]'s region when `id` is the scanner's
    /// synthetic closer of an unmatched begin: the parse reports that primary
    /// bracket error for the pair (at this closer's parked position or its
    /// relocation point), so everything stranded behind it is the spill run
    /// the official enclosing jumps consume silently (`moveSynthetic`
    /// rationale, `parser_impl.dart:5034-5037`). Parser-inserted synthetics
    /// (ids past the table) mark nothing.
    pub(crate) fn mark_spill_run(&mut self, id: crate::token_id::TokenId) {
        if (id as usize) >= self.table.len() {
            return;
        }
        let (_, synthetic) = self.cursor.resolve(id);
        if synthetic {
            self.spill_after = Some(self.span_of(id).range.offset);
        }
    }

    /// Whether the cursor sits in the spill run of a primary bracket error:
    /// a stranded closer or comma strictly behind the scanner's parked
    /// synthetic for an unmatched begin. The mismatching token itself (at
    /// the marked offset) belongs to the enclosing group parses, which
    /// consume it by kind; only what follows is spill. When no mark is
    /// latched yet, [`Self::locate_spill_mark`] scans the consumed prefix.
    pub(crate) fn in_spill_region(&mut self) -> bool {
        if !matches!(
            self.peek_kind(),
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::Comma
        ) {
            return false;
        }
        let head = self.peek_span().range.offset;
        if self.spill_after.is_none_or(|at| head <= at) {
            self.locate_spill_mark(head);
        }
        self.spill_after.is_some_and(|at| head > at)
    }

    /// Backward scan for the scanner's parked synthetic of a primary
    /// bracket error, covering the adoptions that bypass
    /// [`Self::mark_spill_run`] (the argument/list loop heads consume their
    /// closers directly): the consumed table prefix is walked down to
    /// [`Parser::spill_scan_floor`] — the block entry — looking for the
    /// nearest scanner-synthetic closer at or before the head's offset; a
    /// real `{` ends the walk (the block boundary). A find latches
    /// `spill_after`, so later checks are O(1); misses are bounded by
    /// [`SPILL_SCAN_LIMIT`] tokens, keeping repeated stalls linear.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn locate_spill_mark(&mut self, head: u32) {
        #[allow(clippy::cast_possible_truncation)] // ids are u32 by design
        let mut id = self.cursor.real_pos();
        let mut budget = SPILL_SCAN_LIMIT;
        while id > self.spill_scan_floor && budget > 0 {
            id -= 1;
            budget -= 1;
            let tid = id as crate::token_id::TokenId;
            let (kind, synthetic) = self.cursor.resolve(tid);
            let offset = self.span_of(tid).range.offset;
            if offset > head {
                continue; // EOF-parked or later-inserted: no verdict here
            }
            if synthetic
                && matches!(
                    kind,
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly
                )
            {
                self.spill_after = Some(offset);
                return;
            }
            if !synthetic && kind == TokenKind::OpenCurly {
                break; // the enclosing block's `{`: the region ends here
            }
        }
    }

    /// Consumes the spill run of a primary bracket error: the contiguous
    /// stranded closers and commas behind the mismatch. Official consumes
    /// this region through the enclosing group drains — `ensureCloseParen`'s
    /// endGroup jump (:5043-5049), the list drain (:8569-8575), the argument
    /// drain (:9588-9600) — reporting at most the unmatched group's own row,
    /// never one row per token. The run stops at any token an enclosing parse can
    /// still consume (`;`, `}`) or that re-enters parsing (the official
    /// `looksLikeExpressionStart`/`looksLikeLiteralEntry` comma forks,
    /// :9588-9600 / :8563). Structurally advancing: every iteration consumes
    /// one token and EOF pins the cursor.
    pub(crate) fn drain_spill_run(&mut self) {
        while !self.cursor.at_eof() {
            match self.peek_kind() {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::Comma => {
                    self.cursor.advance();
                }
                _ => break,
            }
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

    /// Official `ensureSemicolon` (`parser_impl.dart:5093`): consumes `;` or
    /// reports and synthesizes one.
    ///
    /// Three official details live here. The report anchors on
    /// `findPreviousNonZeroLengthToken(token)` (`parser_impl.dart:5106`,
    /// helper at `:11400`) — the last consumed token, walked back over
    /// zero-width synthetics (an inserted identifier in front of the
    /// insertion point must not capture the anchor) — and the diag spans
    /// that token, so `void f() { x }` reports at the `x`, offset 11
    /// length 1. The message text is the shared `EXPECTED_TOKEN` problem
    /// text `Expected to find ';'.`: the 3.13.1 oracle's machine output for
    /// this face (`dart analyze --format=machine` probe); the 3.15 source
    /// passes `diag.expectedAfterButGot` (`:5107`), which surfaces as the
    /// same analyzer shared code. Finally the inserted `;` drains
    /// everything queued in front of it, so the parse resumes at the token
    /// the official stream carries as `token.next`.
    pub(crate) fn ensure_semicolon(&mut self) -> TokenSpan {
        if let Some(id) = self.cursor.opt_kind(TokenKind::Semicolon) {
            return self.span_of(id);
        }
        let at = self.peek_span();
        let mut anchor = self.last_consumed_anchor();
        // #2024 recovery anchor: a relocation (an adopted scanner closer or
        // the spill mark of a primary bracket error) moves the parse past
        // the walked-back token, and the recovery row anchors on the token
        // the parse resumed at — the official walk-back starts from the
        // construct's last token, which after a relocation is the resume
        // token (upstream Dart SDK source,
        // pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart,
        // `ensureSemicolon`/`rewriteAndRecover` anchor behavior; frozen
        // oracle 0c9fda4b L85 anchors the `;` row on the token following
        // the recovery point).
        if self
            .spill_after
            .is_some_and(|mark| anchor.range.offset <= mark)
        {
            // `findNonZeroLengthToken` over the resume token (upstream Dart
            // SDK source, pkg/_fe_analyzer_shared/lib/src/parser/util.dart):
            // a zero-width synthetic at the resume point must not capture
            // the anchor — the official walk-back never returns a
            // zero-width token.
            let mut n = 0usize;
            let mut forward = at;
            while forward.range.length == 0 && self.cursor.look_kind(n) != TokenKind::Eof {
                n += 1;
                forward = self.look_span_at(n);
            }
            anchor = forward;
        }
        self.report(msg::expected_token(";"), anchor);
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Semicolon, at.range.offset);
        self.cursor.drain_to(id);
        self.span_of(id)
    }

    /// Official report anchor for `reportRecoverableError(token, ...)`: the
    /// last consumed token walked back over zero-width synthetics
    /// (`findPreviousNonZeroLengthToken`, `util.dart:39` — the walk skips only
    /// `isSynthetic && length == 0`, so an inserted identifier in front of the
    /// insertion point must not capture the anchor while a synthetic carrying
    /// real text does: the unterminated string is the official
    /// `SyntheticStringToken` shape (synthetic, non-zero length) and anchors
    /// the ensureSemicolon row on itself — corpus f59c79c8:
    /// `expected_token ';'` at the `"""` string, not the `return`). Falls
    /// back to the current token when nothing non-zero-length was consumed
    /// yet.
    #[allow(clippy::cast_possible_truncation)] // ids are u32 by design
    pub(crate) fn last_consumed_anchor(&self) -> TokenSpan {
        let table_len = self.table.len() as crate::token_id::TokenId;
        let Some(mut id) = self.cursor.last_consumed_id() else {
            return self.peek_span();
        };
        loop {
            let (kind, synthetic) = self.cursor.resolve(id);
            if id >= table_len {
                // Inserted token. Split remainders carry real text and are
                // valid anchors; a consumed synthetic steps to the previous
                // inserted entry, or to the last real token consumed before
                // the insertions.
                if !synthetic {
                    break;
                }
                if id == table_len {
                    if self.cursor.real_pos() == 0 {
                        return self.peek_span();
                    }
                    id = (self.cursor.real_pos() - 1) as crate::token_id::TokenId;
                } else {
                    id -= 1;
                }
                continue;
            }
            // Zero-width synthetics are stepped over; anything carrying real
            // text anchors — synthetic or not (the official walk skips only
            // `isSynthetic && length == 0`). EOF never anchors the walk-back.
            if kind != TokenKind::Eof && self.span_of(id).range.length > 0 {
                break;
            }
            if id == 0 {
                return self.peek_span();
            }
            id -= 1;
        }
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
        // (official ensureBlock: `reportRecoverableError(token, message)`,
        // parser_impl.dart:4995-5002), anchored per `findNonZeroLengthToken`
        // (util.dart:56-61): the last consumed token walked FORWARD over
        // zero-width synthetics — a synthetic name inserted in front of the
        // follower must not capture the anchor (bare `class` at EOF anchors
        // on the EOF position, oracle `issue_46346_prime_2` col 6 length 0),
        // while a real last token carries it (`class C(int a)` at EOF anchors
        // on the `)`). Template flavors (`expectedEnumBody`/`expectedFunction
        // Body`, block_kind.dart:31-34/47-50) report on `next` instead
        // (:5003-5005) — through the SAME `reportRecoverableError`
        // (:11399-11401), so a zero-width synthetic follower walks forward
        // to the next real token too (bare `enum` + a next declaration: the
        // synthetic name inserted behind a reserved head anchors the body
        // row on that head, oracle `issue_48371_prime3` col 1 length 5). The
        // switch-expression lane pins the same last-consumed anchor (its
        // report lands on the `)` of `switch (n);`). The
        // try/catch/finally/switch-statement lanes keep the follower anchor
        // of their own milestones pending their differential passes.
        let follower = self.peek_span();
        let at = match kind {
            crate::message::BlockKind::Class
            | crate::message::BlockKind::Mixin
            | crate::message::BlockKind::Extension
            | crate::message::BlockKind::ExtensionType
            | crate::message::BlockKind::SwitchExpression => match self.cursor.last_consumed_id() {
                Some(id) if self.span_of(id).range.length > 0 => self.span_of(id),
                _ => follower,
            },
            // findNonZeroLengthToken over the follower: forward while
            // zero-width, EOF terminates the walk and stays as-is.
            _ => {
                let mut at = follower;
                let mut n = 0usize;
                while at.range.length == 0 && self.cursor.look_kind(n) != TokenKind::Eof {
                    n += 1;
                    at = self.look_span_at(n);
                }
                at
            }
        };
        let lexeme = self.cursor.lexeme(follower.id);
        self.report(msg::expected_block_body(kind, lexeme), at);
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

    /// Official `recoverFromStackOverflow` (`parser_impl.dart:11386`), the
    /// statement-depth fuse: report `stackOverflow`, insert a synthetic `;`
    /// with its `handleEmptyStatement` event, skip to the enclosing `}` (or
    /// EOF), and latch the terminal bailout so every grammar loop unwinds
    /// without consuming further tokens.
    pub(crate) fn recover_from_stack_overflow(&mut self) {
        if self.is_bailed() {
            return;
        }
        self.bail(msg::stack_overflow(crate::parser::MAX_DEPTH));
        let semi = self.synthetic(TokenKind::Semicolon, self.peek_span().range.offset);
        self.listener.handle_empty_statement(semi);
        while !self.cursor.at_eof() && self.peek_kind() != TokenKind::CloseCurly {
            self.cursor.advance();
        }
    }

    /// Official `parseExpression` stack-overflow recovery
    /// (`parser_impl.dart:6936-6963`), the expression-depth fuse: report
    /// once on the expression start, skip to the upcoming token's
    /// scanner-paired group end — or, when the token carries no group, to
    /// the next `)`/`]`/`}`/`;` — splice a synthetic identifier in front of
    /// it (`insertSyntheticIdentifier` + `handleIdentifier(expression)`),
    /// and return that identifier as the expression.
    ///
    /// Deliberately NOT a terminal bailout: the official parse keeps
    /// running, so the enclosing frames unwind through their closers by
    /// kind and the stranded leftovers fall to the ordinary recovery faces
    /// (`ensureSemicolon`'s walked-back anchor, the top-level invalid
    /// declaration). Those faces are where the oracle's post-fuse rows come
    /// from — oracle `stack_overflow`: `expected_token ';'` on the last
    /// adopted closer, `expected_executable` on the leftover `)`,
    /// `unexpected_token` on the final `;`.
    pub(crate) fn recover_stack_overflow_expression(&mut self) -> TokenSpan {
        let at = self.peek_span();
        self.report(msg::stack_overflow(crate::parser::MAX_DEPTH), at);
        if let Some(end) = self.current_matching_end() {
            self.skip_to_token(end);
        } else {
            while !self.cursor.at_eof()
                && !matches!(
                    self.peek_kind(),
                    TokenKind::CloseParen
                        | TokenKind::CloseSquare
                        | TokenKind::CloseCurly
                        | TokenKind::Semicolon
                )
            {
                self.cursor.advance();
            }
        }
        if self.cursor.at_eof() {
            return self.peek_span();
        }
        // The synthetic head rides the queue in front of the stop token; the
        // single advance consumes it (the missing-name splice's contract), so
        // the cursor sits on the stop token for the unwinding frames.
        let at = self.peek_span();
        let id = self
            .cursor
            .insert_synthetic(TokenKind::Identifier, at.range.offset);
        self.cursor.advance();
        let span = self.span_of(id);
        self.listener
            .handle_identifier(span, IdentifierContext::Expression);
        span
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
        // Typedef names ride the official recovered-identifier template
        // (parseTypedef passes `isIdentifierRecovered = true` for any
        // non-identifier head heading a `(`/`=` group, parser_impl.dart:1494/
        // :1585, so the base recovered variant fires
        // `expectedIdentifierButGotKeyword` for every keyword name — built-in
        // and reserved alike; oracle keyword_named_typedefs.dart 1|14). The
        // second face at the same token (the typedef-name built-in text) is
        // the caller's (`parse_typedef`).
        dartq_scanner::KeywordStyle::BuiltIn
            if context == crate::listener::IdentifierContext::TypedefDeclaration =>
        {
            Some(msg::expected_identifier_but_got_keyword(lexeme))
        }
        _ => None,
    }
}
