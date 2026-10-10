//! The directive family (#300): library/library-augmentation, import,
//! export, part, part-of, and their clause grammar (conditional URIs,
//! import prefixes, show/hide combinators) plus the out-of-order-clause
//! import recovery.
//!
//! Anchors (`.third/dart-sdk/.../parser/parser_impl.dart`, Dart 3.15 main):
//! `parseLibraryAugmentation` :900, `parseLibraryName` :917,
//! `parseImportPrefixOpt` :938, `parseImport` :966, `parseImportRecovery`
//! :995, `parseConditionalUriStar` :1082, `parseConditionalUri` :1095,
//! `parseDottedName` :1151, `parseExport` :1190, `parseCombinatorStar`
//! :1207, `parseHide` :1235, `parseShow` :1248, `parseIdentifierList` :1261,
//! `parsePartOrPartOf` :1304, `parsePart` :1317, `parsePartOf` :1330,
//! `ensureLiteralString` :5075, `parseQualified` :2545, and the directive
//! modifier walks (`modifier_context.dart:183/279`). Directive ordering is
//! the official `DirectiveContext` state machine
//! (`directive_context.dart:12-118`).
//!
//! Import recovery carries the primary parse's clause state forward instead
//! of the official's silent `ImportRecoveryListener` re-probe
//! (`recovery_listeners.dart:39`): the probe re-parses exactly the token
//! range the primary parse just walked to recompute `deferred`/`as`/
//! combinator presence, so the carried state is the same fact without the
//! second walk. Events and diagnostics are identical.

use dartq_scanner::{Keyword, TokenKind};

use crate::listener::{IdentifierContext, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

/// `DirectiveState` (`directive_context.dart:107-115`); the declaration
/// order is load-bearing (`checkLibrary` compares by index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DirectiveState {
    Unknown,
    Script,
    Library,
    ImportAndExport,
    Part,
    PartOf,
    Declarations,
}

impl<L: ParserListener> Parser<'_, '_, L> {
    /// `parseTopLevelKeywordModifiers` (`modifier_context.dart:183`, the
    /// import/export/part walk) and `parseLibraryDirectiveModifiers`
    /// (:279): the shared declaration-modifier walk, with `final` always
    /// extraneous and `augment` captured only for `library` (where it
    /// selects the augmentation form).
    pub(crate) fn parse_directive_modifiers(
        &mut self,
        modifier_start: crate::cursor::CursorMark,
        keyword: TokenSpan,
        capture_augment: bool,
    ) -> Option<TokenSpan> {
        let mods = self.parse_declaration_modifiers(modifier_start, keyword);
        if let Some(f) = mods.final_kw {
            self.report(msg::extraneous_modifier("final"), f);
        }
        match mods.augment {
            Some(a) if capture_augment => Some(a),
            Some(a) => {
                self.report(msg::extraneous_modifier("augment"), a);
                None
            }
            None => None,
        }
    }

    // ------------------------------------------------------------------
    // DirectiveContext (directive_context.dart:29-118)
    // ------------------------------------------------------------------

    /// `checkDeclaration`: any non-directive top-level declaration ends the
    /// directive prelude (a `part of` file keeps that state — nothing may
    /// follow but the `enhanced-parts` relaxation reopens directive mode).
    pub(crate) fn check_declaration(&mut self) {
        if self.directive_state != DirectiveState::PartOf {
            self.directive_state = DirectiveState::Declarations;
        }
    }

    fn check_import_like(
        &mut self,
        token: TokenSpan,
        after_part: crate::message::Message,
        state: DirectiveState,
    ) {
        match self.directive_state {
            DirectiveState::Unknown
            | DirectiveState::Script
            | DirectiveState::Library
            | DirectiveState::ImportAndExport => self.directive_state = state,
            DirectiveState::Part => self.report(after_part, token),
            DirectiveState::PartOf => {
                if self.features.enhanced_parts_enabled {
                    self.directive_state = DirectiveState::ImportAndExport;
                } else {
                    self.report(msg::non_part_of_directive_in_part(), token);
                }
            }
            DirectiveState::Declarations => self.report(msg::directive_after_declaration(), token),
        }
    }

    /// `checkImport` (directive_context.dart:53-72).
    pub(crate) fn check_import(&mut self, token: TokenSpan) {
        self.check_import_like(
            token,
            msg::import_after_part(),
            DirectiveState::ImportAndExport,
        );
    }

    /// `checkExport` (directive_context.dart:37-51).
    pub(crate) fn check_export(&mut self, token: TokenSpan) {
        self.check_import_like(
            token,
            msg::export_after_part(),
            DirectiveState::ImportAndExport,
        );
    }

    /// `checkPart` (directive_context.dart:86-100). Unlike import/export, a
    /// repeated `part` is legal (`Part` admits `Part`).
    pub(crate) fn check_part(&mut self, token: TokenSpan) {
        match self.directive_state {
            DirectiveState::Unknown
            | DirectiveState::Script
            | DirectiveState::Library
            | DirectiveState::ImportAndExport
            | DirectiveState::Part => self.directive_state = DirectiveState::Part,
            DirectiveState::PartOf => {
                if self.features.enhanced_parts_enabled {
                    self.directive_state = DirectiveState::ImportAndExport;
                } else {
                    self.report(msg::non_part_of_directive_in_part(), token);
                }
            }
            DirectiveState::Declarations => self.report(msg::directive_after_declaration(), token),
        }
    }

    /// `checkLibrary` (directive_context.dart:74-84).
    pub(crate) fn check_library(&mut self, token: TokenSpan) {
        if self.directive_state < DirectiveState::Library {
            self.directive_state = DirectiveState::Library;
            return;
        }
        if self.directive_state == DirectiveState::Library {
            self.report(msg::multiple_library_directives(), token);
        } else if self.directive_state == DirectiveState::PartOf {
            self.report(msg::non_part_of_directive_in_part(), token);
        } else {
            self.report(msg::library_directive_not_first(), token);
        }
    }

    /// `checkPartOf` (directive_context.dart:102-117).
    pub(crate) fn check_part_of(&mut self, token: TokenSpan) {
        if self.directive_state == DirectiveState::Unknown {
            self.directive_state = DirectiveState::PartOf;
            return;
        }
        if self.directive_state == DirectiveState::PartOf {
            self.report(msg::part_of_twice(), token);
        } else {
            self.report(msg::non_part_of_directive_in_part(), token);
        }
    }

    // ------------------------------------------------------------------
    // library
    // ------------------------------------------------------------------

    /// Official `parseLibraryAugmentation` :900: `augment library uri;`.
    pub(crate) fn parse_library_augmentation(
        &mut self,
        augment: TokenSpan,
        library_keyword: TokenSpan,
    ) {
        self.listener
            .begin_uncategorized_top_level_declaration(library_keyword);
        self.listener
            .begin_library_augmentation(augment, library_keyword);
        self.cursor.advance(); // `library`
        let _uri = self.ensure_literal_string();
        let semicolon = self.ensure_semicolon();
        self.listener
            .end_library_augmentation(augment, library_keyword, semicolon);
    }

    /// Official `parseLibraryName` :917: `library qualified? ;`.
    pub(crate) fn parse_library_name(&mut self, library_keyword: TokenSpan) {
        self.listener
            .begin_uncategorized_top_level_declaration(library_keyword);
        self.listener.begin_library_name(library_keyword);
        self.cursor.advance(); // `library`
        let has_name = self.peek_kind() != TokenKind::Semicolon;
        let semicolon = if has_name {
            self.parse_qualified(
                IdentifierContext::LibraryName,
                IdentifierContext::LibraryNameContinuation,
            );
            self.ensure_semicolon()
        } else {
            self.ensure_semicolon()
        };
        self.listener
            .end_library_name(library_keyword, semicolon, has_name);
    }

    // ------------------------------------------------------------------
    // import
    // ------------------------------------------------------------------

    /// Official `parseImport` :966:
    /// `import uri (if (test) uri)* importPrefix? combinator* ;`.
    pub(crate) fn parse_import(&mut self) {
        let import_keyword = self.peek_span();
        self.listener
            .begin_uncategorized_top_level_declaration(import_keyword);
        self.listener.begin_import(import_keyword);
        self.cursor.advance(); // `import`
        let _uri = self.ensure_literal_string();
        self.parse_conditional_uri_star();
        let prefix = self.parse_import_prefix_opt();
        let has_combinator = self.parse_combinator_star();
        if self.peek_kind() == TokenKind::Semicolon {
            let semicolon = self.peek_span();
            self.cursor.advance();
            self.listener.end_import(import_keyword, Some(semicolon));
            return;
        }
        // Recovery: out-of-order clauses after the tail (:1004-1007).
        self.listener.end_import(import_keyword, None);
        self.parse_import_recovery(prefix.deferred, prefix.has_prefix(), has_combinator);
    }

    /// Official `parseImportRecovery` :995. `first_deferred`/`has_prefix`/
    /// `has_combinator` carry what the primary parse already saw (see the
    /// module docs for the probe equivalence).
    fn parse_import_recovery(
        &mut self,
        mut first_deferred: Option<TokenSpan>,
        mut has_prefix: bool,
        mut has_combinator: bool,
    ) {
        let mut semicolon: Option<TokenSpan> = None;
        while semicolon.is_none() {
            let start = self.cursor.peek_id();
            self.skip_unexpected_token();
            self.parse_conditional_uri_star();
            // `deferred` not followed by `as`: emit the prefix event for it
            // without recording it (official :1046-1051 — the special case
            // bypasses the recovery listener's bookkeeping).
            if self.peek_kind() == TokenKind::Keyword(Keyword::Deferred)
                && self.cursor.look_kind(1) != TokenKind::Keyword(Keyword::As)
            {
                let deferred = self.peek_span();
                self.listener.handle_import_prefix(Some(deferred), None);
                self.cursor.advance();
            } else {
                let prefix = self.parse_import_prefix_opt();
                if let Some(d) = prefix.deferred {
                    if first_deferred.is_some() {
                        self.report(msg::duplicate_deferred(), d);
                    } else {
                        if has_prefix {
                            self.report(msg::deferred_after_prefix(), d);
                        }
                        first_deferred = Some(d);
                    }
                }
                if let Some(a) = prefix.as_kw {
                    if has_prefix {
                        self.report(msg::duplicate_prefix(), a);
                    } else {
                        if has_combinator {
                            self.report(msg::prefix_after_combinator(), a);
                        }
                        has_prefix = true;
                    }
                }
            }
            let pass_combinator = self.parse_combinator_star();
            has_combinator = has_combinator || pass_combinator;

            if self.peek_kind() == TokenKind::Semicolon {
                let s = self.peek_span();
                self.cursor.advance();
                semicolon = Some(s);
            } else if self.cursor.peek_id() == start {
                // No progress: insert `;` so the loop exits (official
                // :1065-1068). EOF pins the cursor, so this is reached
                // finitely.
                semicolon = Some(self.ensure_semicolon());
            }
            self.listener.handle_recover_import(semicolon);
        }
        if let Some(d) = first_deferred
            && !has_prefix
        {
            self.report(msg::missing_prefix_in_deferred_import(), d);
        }
    }

    /// `skipUnexpectedTokenOpt` :5148 with the import recovery boundary set
    /// (:1022-1025): a stray non-keyword token followed by a clause keyword
    /// (or `;`) is reported and skipped.
    fn skip_unexpected_token(&mut self) {
        if self.cursor.at_eof() {
            return;
        }
        let kind = self.peek_kind();
        if matches!(kind, TokenKind::Keyword(_) | TokenKind::Eof) {
            return;
        }
        let followed_by_clause = matches!(
            self.cursor.look_kind(1),
            TokenKind::Keyword(
                Keyword::If | Keyword::Deferred | Keyword::As | Keyword::Hide | Keyword::Show
            ) | TokenKind::Semicolon
        );
        if followed_by_clause {
            let at = self.peek_span();
            let lexeme = self.cursor.lexeme(at.id).to_owned();
            self.report(msg::unexpected_token(&lexeme), at);
            self.cursor.advance();
        }
    }

    /// Official `parseImportPrefixOpt` :938: `'deferred'? 'as' identifier?`.
    fn parse_import_prefix_opt(&mut self) -> ImportPrefix {
        let next = self.peek_kind();
        if next == TokenKind::Keyword(Keyword::Deferred)
            && self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::As)
        {
            let deferred = self.peek_span();
            self.cursor.advance();
            let as_kw = self.peek_span();
            self.cursor.advance();
            self.ensure_identifier(IdentifierContext::ImportPrefixDeclaration);
            self.listener
                .handle_import_prefix(Some(deferred), Some(as_kw));
            ImportPrefix {
                deferred: Some(deferred),
                as_kw: Some(as_kw),
            }
        } else if next == TokenKind::Keyword(Keyword::As) {
            let as_kw = self.peek_span();
            self.cursor.advance();
            self.ensure_identifier(IdentifierContext::ImportPrefixDeclaration);
            self.listener.handle_import_prefix(None, Some(as_kw));
            ImportPrefix {
                deferred: None,
                as_kw: Some(as_kw),
            }
        } else {
            self.listener.handle_import_prefix(None, None);
            ImportPrefix {
                deferred: None,
                as_kw: None,
            }
        }
    }

    // ------------------------------------------------------------------
    // conditional URIs and dotted names
    // ------------------------------------------------------------------

    /// Official `parseConditionalUriStar` :1082: `conditionalUri*`.
    pub(crate) fn parse_conditional_uri_star(&mut self) {
        let begin = self.peek_span();
        self.listener.begin_conditional_uris(begin);
        let mut count = 0usize;
        while self.peek_kind() == TokenKind::Keyword(Keyword::If) {
            count += 1;
            self.parse_conditional_uri();
        }
        self.listener.end_conditional_uris(count);
    }

    /// Official `parseConditionalUri` :1095:
    /// `if ( dottedName (== string)? ) uri`.
    fn parse_conditional_uri(&mut self) {
        let if_kw = self.peek_span();
        self.cursor.advance(); // `if`
        self.listener.begin_conditional_uri(if_kw);

        let (left_paren, close_id) = if self.peek_kind() == TokenKind::OpenParen {
            let open = self.peek_span();
            self.cursor.advance();
            (open, self.cursor.table_matching_end(open.id))
        } else {
            // insertParens(includeIdentifier: true)
            // (token_stream_rewriter.dart:29): a synthetic
            // `( <empty-id> )` triple at the follower; the grammar then
            // walks it like real tokens.
            let at = self.peek_span();
            let offset = at.range.offset;
            self.report(msg::expected_but_got("("), at);
            let open_id = self.cursor.insert_synthetic(TokenKind::OpenParen, offset);
            let _ = self.cursor.insert_synthetic(TokenKind::Identifier, offset);
            let close_id = self.cursor.insert_synthetic(TokenKind::CloseParen, offset);
            self.cursor.advance(); // synthetic `(`
            (self.span_of(open_id), Some(close_id))
        };

        self.parse_dotted_name();
        let mut equal_sign: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::EqEq {
            equal_sign = Some(self.peek_span());
            self.cursor.advance();
            self.ensure_literal_string();
        }

        if self.peek_kind() != TokenKind::CloseParen
            && let Some(end_id) = close_id
        {
            // The group closes elsewhere: re-report a real closer's
            // intervening token and jump (official :1124-1136; a synthetic
            // closer is adopted silently, `moveSynthetic`).
            let (_, synthetic) = self.cursor.resolve(end_id);
            if !synthetic {
                let at = self.peek_span();
                let lexeme = self.cursor.lexeme(at.id).to_owned();
                self.report(msg::unexpected_token(&lexeme), at);
            }
            self.skip_to_token(end_id);
        }
        let _close = self.expect_kind(TokenKind::CloseParen);

        self.ensure_literal_string();
        self.listener
            .end_conditional_uri(if_kw, left_paren, equal_sign);
    }

    /// Official `parseDottedName` :1151: `identifier ('.' identifier)*`.
    fn parse_dotted_name(&mut self) {
        let first = self.ensure_identifier(IdentifierContext::DottedName);
        let mut count = 1usize;
        while self.peek_kind() == TokenKind::Period {
            self.cursor.advance();
            self.ensure_identifier(IdentifierContext::DottedNameContinuation);
            count += 1;
        }
        self.listener.handle_dotted_name(count, first);
    }

    // ------------------------------------------------------------------
    // export
    // ------------------------------------------------------------------

    /// Official `parseExport` :1190:
    /// `export uri conditionalUri* combinator* ;`.
    pub(crate) fn parse_export(&mut self) {
        let export_keyword = self.peek_span();
        self.listener
            .begin_uncategorized_top_level_declaration(export_keyword);
        self.listener.begin_export(export_keyword);
        self.cursor.advance(); // `export`
        let _uri = self.ensure_literal_string();
        self.parse_conditional_uri_star();
        self.parse_combinator_star();
        let semicolon = self.ensure_semicolon();
        self.listener.end_export(export_keyword, semicolon);
    }

    // ------------------------------------------------------------------
    // combinators
    // ------------------------------------------------------------------

    /// Official `parseCombinatorStar` :1207: `(hideCombinator |
    /// showCombinator)*`. Returns whether any combinator was seen. The
    /// `multipleCombinators` report is gated on the official
    /// `single-combinators` experiment, which dartq does not track (the
    /// oracle lock has it disabled), so no report fires.
    pub(crate) fn parse_combinator_star(&mut self) -> bool {
        let begin = self.peek_span();
        self.listener.begin_combinators(begin);
        let mut count = 0usize;
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Hide) => self.parse_hide(),
                TokenKind::Keyword(Keyword::Show) => self.parse_show(),
                _ => break,
            }
            count += 1;
        }
        self.listener.end_combinators(count);
        count > 0
    }

    /// Official `parseHide` :1235: `hide identifierList`.
    fn parse_hide(&mut self) {
        let hide_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_hide(hide_kw);
        self.parse_identifier_list();
        self.listener.end_hide(hide_kw);
    }

    /// Official `parseShow` :1248: `show identifierList`.
    fn parse_show(&mut self) {
        let show_kw = self.peek_span();
        self.cursor.advance();
        self.listener.begin_show(show_kw);
        self.parse_identifier_list();
        self.listener.end_show(show_kw);
    }

    /// Official `parseIdentifierList` :1261: `identifier (',' identifier)*`.
    fn parse_identifier_list(&mut self) {
        self.ensure_identifier(IdentifierContext::Combinator);
        let mut count = 1usize;
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.ensure_identifier(IdentifierContext::Combinator);
            count += 1;
        }
        self.listener.handle_identifier_list(count);
    }

    // ------------------------------------------------------------------
    // part / part-of
    // ------------------------------------------------------------------

    /// Official `parsePartOrPartOf` :1304: dispatch on the token after
    /// `part`, with the ordering checks before each family.
    pub(crate) fn parse_part_or_part_of(&mut self) {
        let part_keyword = self.peek_span();
        self.listener
            .begin_uncategorized_top_level_declaration(part_keyword);
        if self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::Of) {
            self.check_part_of(part_keyword);
            self.parse_part_of(part_keyword);
        } else {
            self.check_part(part_keyword);
            self.parse_part(part_keyword);
        }
    }

    /// Official `parsePart` :1317: `part uri ;`.
    fn parse_part(&mut self, part_keyword: TokenSpan) {
        self.listener.begin_part(part_keyword);
        self.cursor.advance(); // `part`
        let _uri = self.ensure_literal_string();
        let semicolon = self.ensure_semicolon();
        self.listener.end_part(part_keyword, semicolon);
    }

    /// Official `parsePartOf` :1330: `part of (qualified | uri) ;`.
    fn parse_part_of(&mut self, part_keyword: TokenSpan) {
        let of_kw = self.look_span_at(1);
        self.listener.begin_part_of(part_keyword);
        self.cursor.advance(); // `part`
        self.cursor.advance(); // `of`
        let has_name = self.peek_is_identifier();
        if has_name {
            self.parse_qualified(
                IdentifierContext::PartName,
                IdentifierContext::PartNameContinuation,
            );
        } else {
            self.ensure_literal_string();
        }
        let semicolon = self.ensure_semicolon();
        self.listener
            .end_part_of(part_keyword, of_kw, semicolon, has_name);
    }

    // ------------------------------------------------------------------
    // shared clause helpers
    // ------------------------------------------------------------------

    /// Official `parseQualified` :2545: `identifier ('.' identifier)*` with
    /// distinct first/continuation contexts.
    pub(crate) fn parse_qualified(
        &mut self,
        context: IdentifierContext,
        continuation: IdentifierContext,
    ) {
        self.ensure_identifier(context);
        while self.peek_kind() == TokenKind::Period {
            let period = self.peek_span();
            self.cursor.advance();
            self.ensure_identifier(continuation);
            self.listener.handle_qualified(period);
        }
    }

    /// Official `ensureLiteralString` :5075: the next token must be a string
    /// literal; otherwise report `expectedString` — the ONLY code this
    /// recovery reports (oracle: `import 5;` carries no `unexpectedToken`
    /// from the string read) — and splice a synthetic empty string in BEFORE
    /// the offending token (`rewriteAndRecover` → `insertToken`,
    /// token_stream_rewriter.dart:85 — the real token survives and belongs
    /// to the next grammar position: a following `;` still terminates the
    /// directive, a garbage token is the recovery boundary's or the
    /// top-level continuation's own report), then parse the literal. The
    /// queued synthetic is consumed by [`Self::parse_literal_string`] itself.
    pub(crate) fn ensure_literal_string(&mut self) -> TokenSpan {
        if self.peek_kind() != TokenKind::String {
            let at = self.peek_span();
            let lexeme = self.cursor.lexeme(at.id).to_owned();
            self.report(msg::expected_string(&lexeme), at);
            let _ = self
                .cursor
                .insert_synthetic(TokenKind::String, at.range.offset);
        }
        self.parse_literal_string()
    }
}

/// What `parseImportPrefixOpt` saw (`ImportRecoveryListener.asKeyword`/
/// `deferredKeyword` face).
struct ImportPrefix {
    deferred: Option<TokenSpan>,
    as_kw: Option<TokenSpan>,
}

impl ImportPrefix {
    /// Whether an `as` clause is present.
    fn has_prefix(&self) -> bool {
        self.as_kw.is_some()
    }
}
