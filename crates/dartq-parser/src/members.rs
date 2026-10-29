//! Member grammar (official `parseTopLevelMemberImpl` `parser_impl.dart:4181`,
//! `parseClassOrMixinOrExtensionOrEnumMemberImpl` :5290, `parseFields` :4438,
//! `parseTopLevelMethod` :4606, `parseMethod` :5799, `parseFactoryMethod`
//! :6162, and the full-parser function bodies of #19 — the diet
//! `ClassMemberParser` body mode is fully retired).
//!
//! Function bodies parse through `parse_function_body` (`statements.rs`,
//! official `parseFunctionBody`): `{ ... }` bodies parse the statement
//! grammar, `=> expr;` bodies parse their expression through the full
//! engine, `;` bodies emit the official empty-body event. Field initializers
//! and constructor initializer entries parse through the full engine too
//! (official `parseFieldInitializerOpt`/`parseInitializer`), with the
//! official `mayParseFunctionExpressions = false` rider while the
//! initializer list parses.

use dartq_scanner::{Keyword, KeywordStyle, TokenKind};
use dartq_source::SourceRange;

use crate::listener::{
    AssertKind, ConstructorReferenceContext, DeclarationKind, IdentifierContext, MemberKind,
    ParserListener,
};
use crate::message::msg;
use crate::parser::{AsyncModifier, MAX_DEPTH, Parser};
use crate::recovery::is_identifier;
use crate::token_id::{TokenId, TokenSpan};
use crate::type_args::TypeParamOrArg;
use crate::types::TypeInfo;

/// Hard turn cap for the constructor-initializer recovery loop
/// ([`Parser::parse_initializers_opt`]), a belt over its arm-specific
/// no-progress guard (4× the structural nesting cap — far past anything the
/// parser can even express). The guard only breaks when the real stream is
/// parked AND the inserted-token queue has drained; a recovery state whose
/// every turn re-queues as much as it consumes keeps the queue permanently
/// non-empty, the guard never fires, and the same-anchor reports dedupe to
/// zero diagnostic burn — so even the per-file diagnostics cap never latches
/// and the loop would spin forever. Every legitimate turn consumes at least
/// one real token (the queue is always drained in well-formed parses), so
/// reaching the cap needs thousands of initializer entries in one list:
/// unreachable by valid input.
const INITIALIZER_TURN_CAP: usize = 4 * MAX_DEPTH as usize;

/// A member modifier set as walked by the member impls (official local
/// variables in `parseTopLevelMemberImpl` / `parse...MemberImpl`).
#[derive(Default, Clone, Copy)]
struct MemberModifiers {
    augment: Option<TokenSpan>,
    external: Option<TokenSpan>,
    late_kw: Option<TokenSpan>,
    var_final_or_const: Option<TokenSpan>,
    abstract_kw: Option<TokenSpan>,
    static_kw: Option<TokenSpan>,
    covariant: Option<TokenSpan>,
    var_kw: Option<TokenSpan>,
    final_kw: Option<TokenSpan>,
    const_kw: Option<TokenSpan>,
    required: Option<TokenSpan>,
}

impl<'s, L: ParserListener> Parser<'s, '_, '_, L> {
    // ------------------------------------------------------------------
    // top-level member
    // ------------------------------------------------------------------

    /// Official `parseTopLevelMemberImpl`. The cursor sits before the first
    /// modifier/name token.
    pub(crate) fn parse_top_level_member_impl(&mut self) {
        let before_start = self.cursor.mark();
        self.listener.begin_top_level_member(self.peek_span());

        let mods = self.walk_top_level_member_modifiers();

        let before_type = self.cursor.mark();
        // `var`/`final` pattern variables are illegal at top level; recover
        // as an invalid declaration (official skipOuterPattern + dropRange,
        // replaced by a diet skip of the pattern tokens).
        if let Some(vfc) = mods.var_final_or_const
            && self.cursor.lexeme(vfc.id) != "const"
            && let Some(after_pattern) = self.skip_outer_pattern_id()
        {
            let eq_offset = self.offset_of(after_pattern) + 1;
            if self.kind_at(eq_offset) == TokenKind::Eq {
                let start = self.peek_span();
                let end = self.span_of(after_pattern);
                self.report_with_end(
                    msg::pattern_variable_declaration_outside_function_or_method(),
                    start,
                    end,
                );
                // Skip pattern tokens, the `=`, the RHS, and the `;`
                // (the official drops the pattern range instead).
                self.skip_to_token(after_pattern);
                self.cursor.advance();
                self.cursor.advance(); // `=`
                self.skip_expression_diet();
                let semi = self.ensure_semicolon();
                self.listener.handle_invalid_top_level_declaration(semi);
                return;
            }
        }

        let mut type_info = self.compute_type(false, true);
        type_info.skip_type_raw(self);

        // `get`/`set` introducer before the name.
        let mut get_or_set: Option<TokenSpan> = None;
        if matches!(
            self.peek_kind(),
            TokenKind::Keyword(Keyword::Get | Keyword::Set)
        ) && is_identifier(self.cursor.look_kind(1))
        {
            get_or_set = Some(self.peek_span());
            self.cursor.advance();
        }

        let mut name_is_recovered = false;
        let mut synthetic_name: Option<TokenSpan> = None;
        // Recovery: `<return type>? <reserved word> <method-or-field token>`
        // (official `parseTopLevelMemberImpl` :4304-4320 — the reserved check
        // reads `next.next`, two past the un-consumed type start). The
        // recomputed required type flows into the member parse so no
        // `missing_const_final_var_or_type` rider fires.
        if matches!(type_info, TypeInfo::NoType)
            && mods.var_final_or_const.is_none()
            && self.peek_is_reserved_at(1)
            && self.indicates_method_or_field_at(2)
        {
            type_info = self.compute_type(true, true);
            type_info.skip_type_raw(self);
            name_is_recovered = true;
        }

        // Name-or-special-form dispatch (official `next.type !=
        // TokenType.IDENTIFIER` — a strict identifier-token check, so the
        // built-in-introducer keywords below are still seen).
        if !matches!(self.peek_kind(), TokenKind::Identifier) {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Factory | Keyword::Operator) => {
                    // `factory`/`operator` may be plain identifiers; only
                    // followed by another token that cannot start a member
                    // body do they report.
                    let follower = self.cursor.look_kind(1);
                    if !matches!(
                        follower,
                        TokenKind::OpenParen
                            | TokenKind::OpenCurly
                            | TokenKind::Lt
                            | TokenKind::Function
                            | TokenKind::Eq
                            | TokenKind::Semicolon
                            | TokenKind::Comma
                            | TokenKind::Period
                    ) {
                        let at = self.peek_span();
                        if self.peek_kind() == TokenKind::Keyword(Keyword::Factory) {
                            self.report(msg::factory_top_level_declaration(), at);
                        } else {
                            self.report(msg::top_level_operator(), at);
                        }
                        self.listener.handle_invalid_top_level_declaration(at);
                        self.force_advance();
                        return;
                    }
                }
                kind if !is_keyword_or_identifier(kind) => {
                    if self.cursor.progress()
                        == crate::cursor::TokenCursor::progress_at(before_start)
                    {
                        self.parse_invalid_top_level_declaration();
                        return;
                    }
                    // Declaration missing its identifier: synthesize one and
                    // hand it to the member parse as the name (official
                    // insertSyntheticIdentifier then fall-through — the
                    // synthetic flows into parseFields as the name token,
                    // parser_impl.dart:4367/:4398, so the name slot must not
                    // re-report). The report fires exactly when the
                    // declaration will carry a type or a `var`/`final`/`const`
                    // keyword (official insertSyntheticIdentifier,
                    // parser_impl.dart:4056, is unconditional at this arm;
                    // the probe-pinned 3.13.1 table splits on the fields face:
                    // `var ;`/`final ;` (keyword, no type) and every
                    // committed-type shape (`void<int>? y;`, `void<int> ;`)
                    // report `missing_identifier` at the junk token, while
                    // `late ;`/`int ;` (neither keyword nor committed type —
                    // `computeType(required: false)` only commits when an
                    // identifier follows) stay silent because the junk
                    // becomes the fields face's type-position anchor:
                    // missingConstFinalVarOrType is the only row.
                    let at = self.peek_span();
                    if mods.var_final_or_const.is_some() || !matches!(type_info, TypeInfo::NoType) {
                        self.report(msg::missing_identifier(), at);
                    }
                    let id = self
                        .cursor
                        .insert_synthetic(TokenKind::Identifier, at.range.offset);
                    self.cursor.advance();
                    synthetic_name = Some(self.span_of(id));
                }
                _ => {}
            }
        }

        // Dispatch: the token AFTER the name decides method vs fields
        // (official parser_impl.dart:3946-3953 — a `!` right after the name
        // is skipped for the decision, then `next = next.next!` and the
        // follower value `(`/`{`/`<`/`.`/`=>` selects the method path; the
        // `!` itself is consumed later by `parse_method_type_var`). The
        // cursor stays on the name: `parse_top_level_method` consumes it.
        let follower = if self.cursor.look_kind(1) == TokenKind::Bang {
            self.cursor.look_kind(2)
        } else {
            self.cursor.look_kind(1)
        };
        let is_method = get_or_set.is_some()
            || matches!(
                follower,
                TokenKind::OpenParen
                    | TokenKind::OpenCurly
                    | TokenKind::Lt
                    | TokenKind::Period
                    | TokenKind::Function
            );
        if is_method {
            if let Some(vfc) = mods.var_final_or_const {
                if self.cursor.lexeme(vfc.id) == "var" {
                    self.report(msg::var_return_type(), vfc);
                } else {
                    self.report(msg::extraneous_modifier(self.cursor.lexeme(vfc.id)), vfc);
                }
            }
            if let Some(t) = mods.late_kw {
                self.report(msg::extraneous_modifier("late"), t);
            }
            self.parse_top_level_method(
                before_start,
                mods.augment,
                mods.external,
                before_type,
                type_info,
                get_or_set,
                name_is_recovered,
            );
        } else {
            if let Some(t) = get_or_set {
                self.report(msg::extraneous_modifier(self.cursor.lexeme(t.id)), t);
            }
            self.parse_fields(
                DeclarationKind::TopLevel,
                before_start,
                mods,
                before_type,
                type_info,
                synthetic_name,
                name_is_recovered,
            );
        }
    }

    /// Top-level modifier walk (official head of `parseTopLevelMemberImpl`).
    /// Every arm is gated on the official `isModifier` (modifier_context.dart):
    /// built-in modifier keywords only count when their follower continues a
    /// declaration, so `late;` leaves the keyword in the name slot and the
    /// declaration recovers as a field NAMED `late` (the official
    /// `!next.isIdentifier` fall-through, parser_impl.dart:4356) instead of a
    /// synthetic-named field — the observable difference is resolution of
    /// later bare `late` reads (use_late_in_non_nnbd.dart 19:3).
    fn walk_top_level_member_modifiers(&mut self) -> MemberModifiers {
        let mut mods = MemberModifiers::default();
        let mut vfc_done = false;
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::External)
                    if mods.external.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.augment.is_some() {
                        self.report(msg::modifier_out_of_order("external", "augment"), t);
                    }
                    if mods.var_final_or_const.is_some() {
                        self.report(msg::modifier_out_of_order("external", "var"), t);
                    }
                    mods.external = Some(t);
                }
                TokenKind::Keyword(Keyword::Augment)
                    if mods.augment.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.augment = Some(t);
                }
                TokenKind::Keyword(Keyword::Abstract)
                    if mods.abstract_kw.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    // Official `_parseAbstract` order chain (top-level member
                    // flavor): `var/final/const` before `abstract`.
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("abstract", earlier), t);
                    } else if let Some(c) = mods.covariant {
                        self.report(
                            msg::modifier_out_of_order("abstract", self.cursor.lexeme(c.id)),
                            t,
                        );
                    }
                    self.report(msg::extraneous_modifier("abstract"), t);
                    mods.abstract_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Static) if self.is_modifier_at(0) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("static"), t);
                    mods.static_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Covariant) if self.is_modifier_at(0) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("covariant"), t);
                    mods.covariant = Some(t);
                }
                TokenKind::Keyword(Keyword::Late)
                    if mods.late_kw.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("late", earlier), t);
                    }
                    mods.late_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Var | Keyword::Final | Keyword::Const) if !vfc_done => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.var_final_or_const = Some(t);
                    // `late final` continues; a second var/final/const here
                    // belongs to the next declaration (official comment).
                    if self.peek_kind() == TokenKind::Keyword(Keyword::Final)
                        && self.cursor.lexeme(t.id) != "const"
                        && self.cursor.lexeme(t.id) != "final"
                    {
                        let t2 = self.peek_span();
                        self.cursor.advance();
                        mods.var_final_or_const = Some(t2);
                    }
                    vfc_done = true;
                }
                _ => break,
            }
        }
        mods
    }

    /// Official `parseTopLevelMethod`.
    // `type_info` is consumed by some recovery paths only; the signature
    // mirrors the official `parseTopLevelMethod`.
    #[allow(clippy::needless_pass_by_value)]
    fn parse_top_level_method(
        &mut self,
        before_start: crate::cursor::CursorMark,
        augment: Option<TokenSpan>,
        external: Option<TokenSpan>,
        before_type: crate::cursor::CursorMark,
        type_info: TypeInfo,
        get_or_set: Option<TokenSpan>,
        name_is_recovered: bool,
    ) {
        let last_consumed = self.span_at_mark(before_type);
        self.listener
            .begin_top_level_method(last_consumed, augment, external);

        let begin = self.span_at_mark(before_type);
        let _ = begin;
        // Parse the return type from `before_type`.
        self.cursor.reset(before_type);
        type_info.parse_type(self);

        let mut declared_name: Option<TokenSpan> = None;
        let name = match get_or_set {
            Some(get_set) => {
                // Official `token = getOrSet; ensureIdentifierPotentiallyRecovered`:
                // the identifier event is the name AFTER the `get`/`set`
                // introducer.
                self.cursor.advance();
                let identifier = self
                    .ensure_identifier_recovered(IdentifierContext::TopLevelFunctionDeclaration);
                declared_name = Some(identifier);
                get_set
            }
            None => {
                if name_is_recovered {
                    self.ensure_identifier_recovered(IdentifierContext::TopLevelFunctionDeclaration)
                } else {
                    self.ensure_identifier(IdentifierContext::TopLevelFunctionDeclaration)
                }
            }
        };

        let is_getter = get_or_set.is_some_and(|t| self.cursor.lexeme(t.id) == "get");
        if get_or_set.is_none() {
            let _ = self.parse_method_type_var(name);
        } else {
            self.listener.handle_no_type_variables(self.peek_span());
        }
        let formals_end =
            self.parse_getter_or_formal_parameters(name, is_getter, MemberKind::TopLevelMethod);
        let _ = formals_end;
        // `AstBuilder` `_ensureSetterFormalParameter` guards the top-level
        // setter arm too (ast_builder.dart:6257/:6289): exactly one required
        // positional parameter, anchored on the setter name (measured f18:
        // `set topSetter() => 1;` fires at the identifier). A synthetic
        // empty group (`set s;`) suppresses the row (:6475
        // `!leftParenthesis.isSynthetic`).
        if !is_getter
            && !self.synthetic_formals
            && let Some(setter_name) = declared_name
            && self.last_parameter_count != 1
        {
            self.report(msg::wrong_number_of_parameters_for_setter(), setter_name);
        }

        let saved_async = self.async_state;
        let async_kw = self.parse_async_modifier_opt();
        if get_or_set.is_some_and(|t| self.cursor.lexeme(t.id) == "set")
            && self.async_state != AsyncModifier::Sync
        {
            self.report(msg::setter_not_sync(), async_kw);
        }
        let is_external = external.is_some();
        if is_external && self.peek_kind() != TokenKind::Semicolon {
            self.report(msg::external_method_with_body(), self.peek_span());
        }
        let allow_abstract = is_external || self.features.augmentations_enabled;
        let end = self.parse_function_body(false, allow_abstract);
        self.async_state = saved_async;
        let begin_token = self.span_at_mark(before_start);
        self.listener
            .end_top_level_method(begin_token, get_or_set, end);
    }

    // ------------------------------------------------------------------
    // class / mixin / extension / enum member
    // ------------------------------------------------------------------

    /// Official `parseClassOrMixinOrExtensionOrEnumMemberImpl`.
    pub(crate) fn parse_class_member_impl(
        &mut self,
        kind: DeclarationKind,
        enclosing_declaration_name: Option<&'s str>,
    ) {
        self.parse_metadata_star();

        // Official `beforeStart` (:5295) predates the modifier walk, and
        // `recoverFromInvalidMember`'s no-progress check is
        // `token == beforeStart` (:11352) — a modifier-headed junk member
        // (`late;`) has consumed progress and must fall into `parseFields`'
        // recovery, not `expectedClassMember`.
        let member_start = self.cursor.mark();
        let mods = self.walk_class_member_modifiers();
        // Official `parseClassMemberModifiers`: `required` is always
        // extraneous on a class member (it belongs to named parameters).
        if let Some(r) = mods.required {
            self.report(msg::extraneous_modifier("required"), r);
        }

        self.listener.begin_member();

        let before_type = self.cursor.mark();

        // Pattern variables outside functions (`final (a, b) = ...`) recover
        // as fields with a synthetic name (official synthetic-name +
        // dropRange; the pattern tokens are diet-skipped).
        if let Some(vfc) = mods.var_final_or_const
            && self.cursor.lexeme(vfc.id) != "const"
            && let Some(after_pattern) = self.skip_outer_pattern_id()
        {
            let eq_offset = self.offset_of(after_pattern) + 1;
            if self.kind_at(eq_offset) == TokenKind::Eq {
                let start = self.peek_span();
                let end = self.span_of(after_pattern);
                self.report_with_end(
                    msg::pattern_variable_declaration_outside_function_or_method(),
                    start,
                    end,
                );
                let at = self.peek_span();
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, at.range.offset);
                self.cursor.advance();
                let name = self.span_of(id);
                self.listener
                    .handle_identifier(name, IdentifierContext::FieldDeclaration);
                self.skip_to_token(after_pattern);
                self.cursor.advance();
                self.cursor.advance(); // `=`
                self.skip_expression_diet();
                self.ensure_semicolon();
                self.listener.end_member();
                return;
            }
        }

        let mut type_info = self.compute_type(false, true);
        type_info.skip_type_raw(self);
        let mut after_type = self.cursor.mark();

        // `new` (primary constructor shape) or get/set names.
        let mut new_kw: Option<TokenSpan> = None;
        let mut get_or_set: Option<TokenSpan> = None;
        let mut name_is_recovered = false;

        if self.peek_kind() == TokenKind::Keyword(Keyword::New) {
            new_kw = Some(self.peek_span());
            self.cursor.advance();
        } else if !matches!(self.peek_kind(), TokenKind::Identifier) {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Get | Keyword::Set) => {
                    if is_identifier(self.cursor.look_kind(1)) {
                        get_or_set = Some(self.peek_span());
                        self.cursor.advance();
                    } else if self.peek_is_reserved_at(1) && self.indicates_method_or_field_at(2) {
                        get_or_set = Some(self.peek_span());
                        self.cursor.advance();
                        name_is_recovered = true;
                    }
                }
                TokenKind::Keyword(Keyword::Factory) => {
                    let follower = self.cursor.look_kind(1);
                    if is_identifier(follower)
                        || is_modifier_kind(follower)
                        || follower == TokenKind::Keyword(Keyword::New)
                    {
                        // `typeBeforeFactory` gates on a type actually having
                        // been consumed (official `beforeType != token`,
                        // parser_impl.dart:5448).
                        if crate::cursor::TokenCursor::progress_at(before_type)
                            != crate::cursor::TokenCursor::progress_at(after_type)
                        {
                            let t = self.peek_span();
                            self.report(msg::type_before_factory(), t);
                        }
                        if let Some(a) = mods.abstract_kw {
                            self.report(msg::abstract_class_member(), a);
                        }
                        self.parse_factory_method(
                            kind,
                            mods.augment,
                            mods.external,
                            mods.static_kw.or(mods.covariant),
                            mods.var_final_or_const,
                            true,
                        );
                        self.listener.end_member();
                        return;
                    }
                    // Unnamed factory (`factory ()`) — primary-constructors
                    // shape (official :5456-5477): only with a clean modifier
                    // set (no consumed type, no covariant/late/static, no
                    // abstract, `const` or no var/final modifier). Otherwise
                    // fall through: `factory` used as an identifier.
                    if follower == TokenKind::OpenParen
                        && self.features.primary_constructors_enabled
                        && crate::cursor::TokenCursor::progress_at(before_type)
                            == crate::cursor::TokenCursor::progress_at(after_type)
                        && mods.covariant.is_none()
                        && mods.late_kw.is_none()
                        && mods.static_kw.is_none()
                        && mods.abstract_kw.is_none()
                        && mods
                            .var_final_or_const
                            .is_none_or(|t| self.cursor.lexeme(t.id) == "const")
                    {
                        self.parse_factory_method(
                            kind,
                            mods.augment,
                            mods.external,
                            None,
                            mods.var_final_or_const,
                            false,
                        );
                        self.listener.end_member();
                        return;
                    }
                    // Fall through: `factory` used as an identifier.
                }
                TokenKind::Keyword(Keyword::Operator) => {
                    // Official parseMember `operator` branch (:5489-5504):
                    // arm 1 requires `typeParam == noTypeParamOrArg`, i.e. no
                    // type-argument group directly after `operator`
                    // (`operator <T>(...)` falls through; `operator <(x)`
                    // still parses as the `<` operator).
                    let follower = self.cursor.look_kind(1);
                    let type_param_none = follower != TokenKind::Lt
                        || self.cursor.look_kind(2) == TokenKind::OpenParen;
                    if Self::is_user_definable_operator(follower) && type_param_none {
                        self.parse_method(
                            kind,
                            enclosing_declaration_name,
                            mods,
                            before_type,
                            type_info,
                            None,
                            None,
                            name_is_recovered,
                        );
                        self.listener.end_member();
                        return;
                    }
                    if matches!(follower, TokenKind::EqEqEq | TokenKind::BangEqEq)
                        || (Self::is_operator_token(follower)
                            && follower != TokenKind::Eq
                            && follower != TokenKind::Lt)
                    {
                        self.parse_invalid_operator_declaration(kind, mods, before_type);
                        self.listener.end_member();
                        return;
                    }
                    // Fall through: `operator` used as an identifier.
                }
                TokenKind::Keyword(Keyword::This) => {
                    // Primary constructor body (`this : ...` / `this(...)`).
                    let follower = self.cursor.look_kind(1);
                    if matches!(
                        follower,
                        TokenKind::Colon
                            | TokenKind::Semicolon
                            | TokenKind::OpenCurly
                            | TokenKind::Function
                    ) || matches!(follower, TokenKind::Keyword(Keyword::Async | Keyword::Sync))
                    {
                        if let Some(t) = mods.var_final_or_const {
                            self.report(msg::extraneous_modifier(self.cursor.lexeme(t.id)), t);
                        }
                        if let Some(t) = mods.external {
                            self.report(msg::extraneous_modifier("external"), t);
                        }
                        if let Some(t) = mods.static_kw {
                            self.report(msg::extraneous_modifier("static"), t);
                        }
                        if let Some(t) = mods.covariant {
                            self.report(msg::extraneous_modifier("covariant"), t);
                        }
                        if let Some(t) = mods.late_kw {
                            self.report(msg::extraneous_modifier("late"), t);
                        }
                        self.cursor.advance(); // `this`
                        self.parse_primary_constructor_body();
                        self.listener.end_member();
                        return;
                    }
                }
                ref k if !is_keyword_or_identifier(*k) => {
                    if let Some(a) = mods.abstract_kw {
                        self.report(msg::abstract_class_member(), a);
                    }
                    self.recover_from_invalid_member(
                        kind,
                        enclosing_declaration_name,
                        mods,
                        member_start,
                        before_type,
                        type_info,
                        get_or_set,
                        new_kw,
                    );
                    self.listener.end_member();
                    return;
                }
                TokenKind::Keyword(Keyword::Class | Keyword::Enum) => {
                    // Official parseMember catch-all (`!next.isIdentifier`):
                    // `class`/`enum` at member start route to
                    // `recoverFromInvalidMember` (classInClass/enumInClass).
                    if let Some(a) = mods.abstract_kw {
                        self.report(msg::abstract_class_member(), a);
                    }
                    self.recover_from_invalid_member(
                        kind,
                        enclosing_declaration_name,
                        mods,
                        member_start,
                        before_type,
                        type_info,
                        get_or_set,
                        new_kw,
                    );
                    self.listener.end_member();
                    return;
                }
                TokenKind::Keyword(Keyword::Typedef)
                    if matches!(type_info, TypeInfo::NoType)
                        && is_identifier(self.cursor.look_kind(1)) =>
                {
                    // Official routing gate for `typedef`
                    // (parser_impl.dart:5607): no type and a name follows.
                    if let Some(a) = mods.abstract_kw {
                        self.report(msg::abstract_class_member(), a);
                    }
                    self.recover_from_invalid_member(
                        kind,
                        enclosing_declaration_name,
                        mods,
                        member_start,
                        before_type,
                        type_info,
                        get_or_set,
                        new_kw,
                    );
                    self.listener.end_member();
                    return;
                }
                TokenKind::Keyword(kw) if kw.style() == KeywordStyle::Reserved => {
                    // Official :5606: every non-identifier head that none of
                    // the special cases handled recovers as an invalid
                    // member (`null`, `super`, `this` outside a primary
                    // constructor shape, ...).
                    if let Some(a) = mods.abstract_kw {
                        self.report(msg::abstract_class_member(), a);
                    }
                    self.recover_from_invalid_member(
                        kind,
                        enclosing_declaration_name,
                        mods,
                        member_start,
                        before_type,
                        type_info,
                        get_or_set,
                        new_kw,
                    );
                    self.listener.end_member();
                    return;
                }
                _ => {}
            }
        } else if matches!(type_info, TypeInfo::NoType) && mods.var_final_or_const.is_none() {
            // Missing `operator` keyword before a user-definable operator, or
            // a reserved word used as the name.
            let follower = self.cursor.look_kind(1);
            if Self::is_user_definable_operator(follower) {
                let after_op = self.cursor.look_kind(2);
                if matches!(
                    after_op,
                    TokenKind::OpenParen | TokenKind::OpenCurly | TokenKind::Function
                ) {
                    self.parse_invalid_operator_declaration(kind, mods, before_type);
                    self.listener.end_member();
                    return;
                }
            } else if self.peek_is_reserved_at(1) && self.indicates_method_or_field_at(2) {
                // Recovery: use the reserved keyword as the name (official
                // :5651-5668); the recomputed required type must flow into
                // the member parse, or a `missing_const_final_var_or_type`
                // rider fires on every keyword-named member.
                type_info = self.compute_type(true, true);
                type_info.skip_type_raw(self);
                after_type = self.cursor.mark();
                name_is_recovered = true;
            }
        }

        // Method vs fields: the token AFTER the name decides (official
        // parser_impl.dart:5214-5225 — `next = next.next!` past the name,
        // then the follower value `(`/`{`/`<`/`.`/`=>`; no `!` recovery
        // here, unlike the top-level dispatch). The cursor stays on the
        // name: `parse_method` consumes it.
        let follower = self.cursor.look_kind(1);
        let is_method = get_or_set.is_some()
            || new_kw.is_some()
            || matches!(
                follower,
                TokenKind::OpenParen
                    | TokenKind::OpenCurly
                    | TokenKind::Lt
                    | TokenKind::Period
                    | TokenKind::Function
            );
        if is_method {
            self.parse_method(
                kind,
                enclosing_declaration_name,
                mods,
                before_type,
                type_info,
                get_or_set,
                new_kw,
                name_is_recovered,
            );
        } else {
            if let Some(t) = get_or_set {
                self.report(msg::extraneous_modifier(self.cursor.lexeme(t.id)), t);
            }
            self.parse_fields(
                kind,
                after_type,
                mods,
                before_type,
                type_info,
                None,
                name_is_recovered,
            );
        }
        self.listener.end_member();
    }

    /// Class-member modifier walk (official `ModifierContext._parseModifiers`
    /// handlers + `parseClassMemberModifiers`). Every arm mirrors its
    /// official `_parse<Key>` handler: order checks against already-seen
    /// modifiers, then the duplicate/recovery reports. Built-in modifier
    /// arms are gated on the official `isModifier` follower rule
    /// (modifier_context.dart:13-53), so `late;` inside a class recovers as
    /// a field NAMED `late` (official `!next.isIdentifier` fall-through)
    /// instead of a synthetic-named field.
    fn walk_class_member_modifiers(&mut self) -> MemberModifiers {
        let mut mods = MemberModifiers::default();
        let mut static_seen = false;
        let mut covariant_seen = false;
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::External)
                    if mods.external.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    // Official `_parseExternal` order chain (no augment arm).
                    if let Some(c) = mods.const_kw {
                        let earlier = self.cursor.lexeme(c.id);
                        self.report(msg::modifier_out_of_order("external", earlier), t);
                    } else if mods.static_kw.is_some() {
                        self.report(msg::modifier_out_of_order("external", "static"), t);
                    } else if mods.late_kw.is_some() {
                        self.report(msg::modifier_out_of_order("external", "late"), t);
                    } else if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("external", earlier), t);
                    } else if mods.covariant.is_some() {
                        self.report(msg::modifier_out_of_order("external", "covariant"), t);
                    }
                    mods.external = Some(t);
                }
                TokenKind::Keyword(Keyword::Augment)
                    if mods.augment.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    // Official `_parseAugment` order chain.
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("augment", earlier), t);
                    } else if let Some(a) = mods.abstract_kw {
                        self.report(
                            msg::modifier_out_of_order("augment", self.cursor.lexeme(a.id)),
                            t,
                        );
                    } else if mods.const_kw.is_some() {
                        self.report(msg::modifier_out_of_order("augment", "const"), t);
                    } else if mods.covariant.is_some() {
                        self.report(msg::modifier_out_of_order("augment", "covariant"), t);
                    } else if mods.final_kw.is_some() {
                        self.report(msg::modifier_out_of_order("augment", "final"), t);
                    } else if mods.late_kw.is_some() {
                        self.report(msg::modifier_out_of_order("augment", "late"), t);
                    } else if mods.static_kw.is_some() {
                        self.report(msg::modifier_out_of_order("augment", "static"), t);
                    } else if mods.external.is_some() {
                        self.report(msg::modifier_out_of_order("augment", "external"), t);
                    }
                    mods.augment = Some(t);
                }
                TokenKind::Keyword(Keyword::Abstract)
                    if mods.abstract_kw.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    // Official `_parseAbstract` order chain.
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("abstract", earlier), t);
                    } else if let Some(c) = mods.covariant {
                        self.report(
                            msg::modifier_out_of_order("abstract", self.cursor.lexeme(c.id)),
                            t,
                        );
                    }
                    mods.abstract_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Static) if self.is_modifier_at(0) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if !covariant_seen && !static_seen {
                        // Official `_parseStatic` order chain.
                        if mods.abstract_kw.is_some() && self.features.augmentations_enabled {
                            self.report(msg::modifier_out_of_order("static", "abstract"), t);
                        } else if mods.const_kw.is_some() {
                            self.report(msg::modifier_out_of_order("static", "const"), t);
                        } else if mods.final_kw.is_some() {
                            self.report(msg::modifier_out_of_order("static", "final"), t);
                        } else if mods.late_kw.is_some() {
                            self.report(msg::modifier_out_of_order("static", "late"), t);
                        } else if mods.var_kw.is_some() {
                            self.report(msg::modifier_out_of_order("static", "var"), t);
                        }
                        mods.static_kw = Some(t);
                        static_seen = true;
                    } else if covariant_seen {
                        // Recovery: static after covariant.
                        self.report(msg::covariant_and_static(), t);
                    } else {
                        self.report(msg::duplicated_modifier("static"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Covariant) if self.is_modifier_at(0) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if !covariant_seen && !static_seen && mods.const_kw.is_none() {
                        // Official `_parseCovariant` order chain.
                        if let Some(v) = mods.var_kw {
                            self.report(
                                msg::modifier_out_of_order("covariant", self.cursor.lexeme(v.id)),
                                t,
                            );
                        } else if mods.final_kw.is_some() {
                            self.report(msg::modifier_out_of_order("covariant", "final"), t);
                        } else if mods.late_kw.is_some() {
                            self.report(msg::modifier_out_of_order("covariant", "late"), t);
                        }
                        mods.covariant = Some(t);
                        covariant_seen = true;
                    } else if covariant_seen {
                        self.report(msg::duplicated_modifier("covariant"), t);
                    } else if static_seen {
                        self.report(msg::covariant_and_static(), t);
                    } else {
                        // covariant after const.
                        self.report(msg::conflicting_modifiers("covariant", "const"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Late)
                    if mods.late_kw.is_none() && self.is_modifier_at(0) =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    // Official `_parseLate` order chain.
                    if mods.const_kw.is_some() {
                        self.report(msg::conflicting_modifiers("late", "const"), t);
                    } else if mods.var_kw.is_some() {
                        self.report(msg::modifier_out_of_order("late", "var"), t);
                    } else if mods.final_kw.is_some() {
                        self.report(msg::modifier_out_of_order("late", "final"), t);
                    }
                    mods.late_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Final) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.final_kw.is_none() && mods.var_final_or_const.is_none() {
                        mods.final_kw = Some(t);
                        mods.var_final_or_const = Some(t);
                    } else if mods.final_kw.is_some() {
                        self.report(msg::duplicated_modifier("final"), t);
                    } else if mods.const_kw.is_some() {
                        self.report(msg::const_and_final(), t);
                    } else if mods.var_kw.is_some() {
                        self.report(msg::final_and_var(), t);
                    } else if mods.late_kw.is_some() {
                        self.report(msg::modifier_out_of_order("final", "late"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Var) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.var_final_or_const.is_none() {
                        mods.var_kw = Some(t);
                        mods.var_final_or_const = Some(t);
                    } else if mods.var_kw.is_some() {
                        self.report(msg::duplicated_modifier("var"), t);
                    } else if mods.const_kw.is_some() {
                        self.report(msg::conflicting_modifiers("var", "const"), t);
                    } else if mods.final_kw.is_some() {
                        self.report(msg::final_and_var(), t);
                    }
                }
                TokenKind::Keyword(Keyword::Const) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.var_final_or_const.is_none() && mods.covariant.is_none() {
                        // Official `_parseConst`: const is recorded; `late`
                        // conflicts, `final` reports const_and_final.
                        if mods.late_kw.is_some() {
                            self.report(msg::conflicting_modifiers("const", "late"), t);
                        }
                        mods.const_kw = Some(t);
                        mods.var_final_or_const = Some(t);
                    } else if mods.const_kw.is_some() {
                        self.report(msg::duplicated_modifier("const"), t);
                    } else if mods.covariant.is_some() {
                        self.report(msg::conflicting_modifiers("const", "covariant"), t);
                    } else if mods.final_kw.is_some() {
                        self.report(msg::const_and_final(), t);
                    } else if mods.var_kw.is_some() {
                        self.report(msg::conflicting_modifiers("const", "var"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Required) if self.is_modifier_at(0) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    // Official `_parseRequired` order chain (the member-level
                    // extraneous report happens in `parse_class_member_impl`
                    // via `parseClassMemberModifiers`).
                    if let Some(c) = mods.const_kw {
                        self.report(
                            msg::modifier_out_of_order("required", self.cursor.lexeme(c.id)),
                            t,
                        );
                    } else if let Some(c) = mods.covariant {
                        self.report(
                            msg::modifier_out_of_order("required", self.cursor.lexeme(c.id)),
                            t,
                        );
                    } else if mods.final_kw.is_some() {
                        self.report(msg::modifier_out_of_order("required", "final"), t);
                    } else if mods.var_kw.is_some() {
                        self.report(msg::modifier_out_of_order("required", "var"), t);
                    }
                    mods.required = Some(t);
                }
                _ => break,
            }
        }
        mods
    }

    /// Official `parseMethod` (methods, getters, setters, operators,
    /// constructors).
    // `type_info` is consumed by some recovery paths only; the signature
    // mirrors the official `parseMethod`.
    #[allow(clippy::needless_pass_by_value)]
    fn parse_method(
        &mut self,
        kind: DeclarationKind,
        enclosing_declaration_name: Option<&'s str>,
        mods: MemberModifiers,
        before_type: crate::cursor::CursorMark,
        type_info: TypeInfo,
        get_or_set: Option<TokenSpan>,
        new_kw: Option<TokenSpan>,
        name_is_recovered: bool,
    ) {
        let mut static_kw = mods.static_kw;
        let mut covariant = mods.covariant;
        let mut var_final_or_const = mods.var_final_or_const;

        if let Some(a) = mods.abstract_kw {
            self.report(msg::abstract_class_member(), a);
        }
        if let Some(t) = mods.late_kw {
            self.report(msg::extraneous_modifier("late"), t);
        }

        // `operator <op>` names (official parseMethod `isOperator` gate +
        // `parseOperatorName` sub-cases).
        let mut is_operator = false;
        let mut name: TokenSpan;
        if get_or_set.is_none() && self.peek_kind() == TokenKind::Keyword(Keyword::Operator) {
            let operator_kw = self.peek_span();
            let follower = self.cursor.look_kind(1);
            // Official :5826: `operator.isOperator || === || !== ||
            // isUnaryMinus` — the full operator-token set, not just the
            // user-definable one (`operator = 2` is a field, recovered as an
            // invalid operator declaration from `recoverFromInvalidMember`).
            if Self::is_operator_token(follower)
                || matches!(follower, TokenKind::EqEqEq | TokenKind::BangEqEq)
            {
                is_operator = true;
                self.cursor.advance();
                let op = self.peek_span();
                self.cursor.advance();
                if matches!(follower, TokenKind::EqEqEq | TokenKind::BangEqEq) {
                    // The scanner reports `===`/`!==`; the parser only fires
                    // the invalid-name event (official parseOperatorName).
                    self.listener.handle_invalid_operator_name(operator_kw, op);
                } else if !Self::is_user_definable_operator(follower) {
                    // Official parseOperatorName (:6303-6312): the user has
                    // specified an invalid operator name; report and accept.
                    self.report(msg::invalid_operator(self.cursor.lexeme(op.id)), op);
                    self.listener.handle_invalid_operator_name(operator_kw, op);
                } else {
                    self.listener.handle_operator_name(operator_kw, op);
                }
                name = op;
            } else if is_unary_minus_ahead(self) {
                // Legacy `operator unary-(` form: recovery (official
                // parseOperatorName isUnaryMinus branch).
                is_operator = true;
                let unary = self.peek_span();
                self.cursor.advance();
                self.report(msg::unexpected_token(self.cursor.lexeme(unary.id)), unary);
                let minus = self.peek_span();
                self.cursor.advance();
                self.listener.handle_operator_name(operator_kw, minus);
                name = minus;
            } else {
                // `operator` is a plain identifier name (`int operator = 2`,
                // `int operator<T>()`).
                name = operator_kw;
                self.cursor.advance();
            }
        } else {
            name = self.peek_span();
        }

        let is_constructor = self.is_constructor(
            name,
            get_or_set,
            new_kw,
            enclosing_declaration_name,
            is_operator,
        );

        if let Some(s) = static_kw {
            if is_operator {
                self.report(msg::static_operator(), s);
                static_kw = None;
            }
        } else if let Some(c) = covariant
            && (get_or_set.is_none() || self.cursor.lexeme(c.id) == "get")
        {
            self.report(msg::covariant_member(), c);
            covariant = None;
        }
        if let Some(vfc) = var_final_or_const {
            match self.cursor.lexeme(vfc.id) {
                "const" => {
                    if get_or_set.is_some() {
                        self.report(msg::extraneous_modifier("const"), vfc);
                        var_final_or_const = None;
                    }
                }
                "var" => {
                    self.report(msg::var_return_type(), vfc);
                    var_final_or_const = None;
                }
                _ => {
                    self.report(msg::extraneous_modifier("final"), vfc);
                    var_final_or_const = None;
                }
            }
        }

        if is_constructor {
            self.listener.begin_constructor(
                kind,
                mods.augment,
                mods.external,
                static_kw,
                covariant,
                var_final_or_const,
                get_or_set,
                new_kw,
                name,
                enclosing_declaration_name,
            );
        } else {
            // Member-name event gate: a begin-method name is a declared-name
            // token — the identifier event's token (identifier, or the
            // reserved word `ensureIdentifierPotentiallyRecovered` accepted).
            // The `recoverFromInvalidMember` dispatch hands non-lexical heads
            // (`{`, `(`, `=>` — e.g. a constructor body orphaned by the
            // `this.new` initializer cascade re-entering the member loop,
            // parser_impl.dart:11334) through here with the raw token, but
            // the official builder never names from it: the name comes from
            // `ensureIdentifierPotentiallyRecovered`, which reports
            // `missing_identifier` and names the member with the inserted
            // synthetic. Operators are named by `handleOperatorName` and keep
            // their operator token. Minting the synthetic face queues it
            // ahead of the junk head and drains it immediately — no stream
            // movement, no report; the return-type re-parse's
            // `reset(before_type)` then truncates the queued entry, and the
            // name resolution below re-issues the same id for its own
            // recovery synthetic (ids mint as `table_len + inserted_len`,
            // rewound by the reset), so the face resolves to exactly the
            // zero-width synthetic the official declared name carries.
            let name = if is_operator || is_keyword_or_identifier(self.peek_kind()) {
                name
            } else {
                let at = self.peek_span();
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, at.range.offset);
                self.cursor.advance(); // drain the minted synthetic; the junk head stays current
                self.span_of(id)
            };
            self.listener.begin_method(
                kind,
                mods.augment,
                mods.external,
                static_kw,
                covariant,
                var_final_or_const,
                get_or_set,
                name,
                enclosing_declaration_name,
            );
        }

        // Return type.
        self.cursor.reset(before_type);
        type_info.parse_type(self);
        if let Some(gs) = get_or_set {
            // Official `token = getOrSet; ensureIdentifierPotentiallyRecovered`:
            // the identifier event is the name AFTER the `get`/`set`
            // introducer. With no return type the cursor already sits on the
            // introducer (`jump_to` requires strictly-forward movement).
            if (gs.id as usize) > self.cursor.real_pos() {
                self.cursor.jump_to(gs.id);
            }
            self.cursor.advance();
        } else if is_operator {
            // The return-type re-parse rewound to `operator`; skip the
            // introducer and the operator token consumed above (grammar
            // guarantees adjacency, official parseMethod keeps `token` on
            // the name throughout). The recovered no-keyword form (synthetic
            // introducer) may already sit ON the operator token.
            if (name.id as usize) > self.cursor.real_pos() {
                self.cursor.jump_to(name.id);
            }
            self.cursor.advance();
        }

        let mut has_qualified_name = false;
        if is_operator {
            // Name already consumed above.
        } else if new_kw.is_some() {
            // Official `token = token.next!` (:5936): the checks read the
            // token AFTER the `new` head, which itself is the constructor
            // name carried by `newToken`.
            self.cursor.advance();
            if is_identifier(self.peek_kind()) {
                let ident = self.ensure_identifier(IdentifierContext::MethodDeclaration);
                name = ident;
                let before = self.cursor.mark();
                self.parse_qualified_rest_opt(IdentifierContext::MethodDeclarationContinuation);
                if self.cursor.progress() != crate::cursor::TokenCursor::progress_at(before) {
                    has_qualified_name = true;
                    self.report(msg::new_constructor_qualified_name(), ident);
                }
            } else if self.peek_kind() == TokenKind::Keyword(Keyword::New) {
                let ident = self.peek_span();
                self.report(msg::new_constructor_new_name(), ident);
                self.cursor.advance();
                self.listener
                    .handle_identifier(ident, IdentifierContext::MethodDeclaration);
                name = ident;
            } else if self.peek_kind() == TokenKind::Period
                && is_identifier(self.cursor.look_kind(1))
            {
                let dot = self.peek_span();
                self.report(msg::new_constructor_dot_name(), dot);
                self.cursor.advance();
                let ident = self.ensure_identifier(IdentifierContext::MethodDeclaration);
                name = ident;
            } else {
                let at = self.peek_span();
                self.listener
                    .handle_no_identifier(at, IdentifierContext::MethodDeclaration);
                // Official parseMethod keeps `name = newToken`: with no
                // identifier after the `new` head, the keyword itself is the
                // unnamed-constructor name, so the mmp recovery anchor
                // (parseGetterOrFormalParameters) points at the rejected
                // `new` token, never at its follower (`new = null;` in a
                // class body: oracle `missing_method_parameters` on `new`).
                name = new_kw.expect("new_kw arm guarantees the head");
            }
        } else {
            name = if name_is_recovered {
                self.ensure_identifier_recovered(IdentifierContext::MethodDeclaration)
            } else {
                self.ensure_identifier(IdentifierContext::MethodDeclaration)
            };
            let before = self.cursor.mark();
            self.parse_qualified_rest_opt(IdentifierContext::MethodDeclarationContinuation);
            if self.cursor.progress() != crate::cursor::TokenCursor::progress_at(before) {
                has_qualified_name = true;
            }
        }

        // Type variables (unless get/set).
        let mut is_considered_getter = false;
        if let Some(gos) = get_or_set {
            is_considered_getter = self.cursor.lexeme(gos.id) == "get";
            self.listener.handle_no_type_variables(self.peek_span());
            if has_qualified_name || (is_considered_getter && self.peek_kind() == TokenKind::Colon)
            {
                is_considered_getter = false;
            }
        } else {
            let lt = if self.peek_kind() == TokenKind::Lt {
                Some(self.peek_span())
            } else {
                None
            };
            let type_vars = self.parse_method_type_var(name);
            if let (Some(lt), Some(closer)) = (lt, type_vars) {
                // Upstream anchors at the type-parameter LIST node
                // (`outline_builder.dart:2492/2626/4551`: `addProblem` with
                // `typeParameters.charOffset` + `typeParameters.spanLength`)
                // — `<T>` in full, not the bare `<` (manual face
                // type_parameter_on_constructor/1: 2:4|3).
                if is_constructor {
                    self.report_with_end(msg::type_parameter_on_constructor(), lt, closer);
                } else if is_operator {
                    self.report_with_end(msg::type_parameter_on_operator(), lt, closer);
                }
            }
        }

        let member_kind = match kind {
            DeclarationKind::Extension => {
                if static_kw.is_some() {
                    MemberKind::ExtensionStaticMethod
                } else {
                    MemberKind::ExtensionNonStaticMethod
                }
            }
            DeclarationKind::ExtensionType => {
                if static_kw.is_some() {
                    MemberKind::ExtensionTypeStaticMethod
                } else {
                    MemberKind::ExtensionTypeNonStaticMethod
                }
            }
            _ => {
                if static_kw.is_some() {
                    MemberKind::StaticMethod
                } else {
                    MemberKind::NonStaticMethod
                }
            }
        };

        let name_for_formals = name;
        self.external_ctor_field_formals = is_constructor && mods.external.is_some();
        self.in_constructor_formals = is_constructor;
        let begin_param = self.parse_getter_or_formal_parameters(
            name_for_formals,
            is_considered_getter,
            member_kind,
        );
        self.external_ctor_field_formals = false;
        self.in_constructor_formals = false;
        // `AstBuilder` `_ensureSetterFormalParameter` (ast_builder.dart:6457):
        // a setter takes exactly one required positional parameter; the
        // report anchors on the setter name. A synthetic empty group
        // (`set s;`) suppresses the row (:6475 `!leftParenthesis.isSynthetic`).
        if !is_constructor
            && !is_considered_getter
            && get_or_set.is_some_and(|t| self.cursor.lexeme(t.id) == "set")
            && !self.synthetic_formals
            && self.last_parameter_count != 1
        {
            self.report(msg::wrong_number_of_parameters_for_setter(), name);
        }

        let begin_initializers = self.parse_initializers_opt();

        let saved_async = self.async_state;
        let async_kw = self.parse_async_modifier_opt();
        if get_or_set.is_some_and(|t| self.cursor.lexeme(t.id) == "set")
            && self.async_state != AsyncModifier::Sync
        {
            self.report(msg::setter_not_sync(), async_kw);
        }
        if mods.external.is_some() && self.peek_kind() != TokenKind::Semicolon {
            self.report(msg::external_method_with_body(), self.peek_span());
        }
        let body_kind = self.peek_kind();
        let body_begin = self.peek_span();
        let end = if self.peek_kind() == TokenKind::Eq {
            let at = self.peek_span();
            self.report(msg::redirection_in_non_factory(), at);
            self.parse_redirecting_factory_body()
        } else {
            let allow_abstract = (static_kw.is_none()
                || mods.external.is_some()
                || self.features.augmentations_enabled)
                && self.async_state == AsyncModifier::Sync;
            self.parse_function_body(false, allow_abstract)
        };
        self.async_state = saved_async;

        if is_constructor {
            if new_kw.is_none() {
                let name_matches = enclosing_declaration_name == Some(self.cursor.lexeme(name.id));
                if !name_matches {
                    self.report(msg::invalid_constructor_name(), name);
                }
            }
            if let Some(s) = static_kw {
                self.report(msg::static_constructor(), s);
            }
            if let Some(gs) = get_or_set {
                if self.cursor.lexeme(gs.id) == "get" {
                    self.report(msg::getter_constructor(), gs);
                } else {
                    self.report(msg::setter_constructor(), gs);
                }
            }
            if !matches!(type_info, TypeInfo::NoType) {
                let start = self.span_at_mark(before_type);
                self.report(msg::constructor_with_return_type(), start);
            }
            if begin_initializers.is_some() && mods.external.is_some() {
                self.report(
                    msg::external_constructor_with_initializer(),
                    self.peek_span(),
                );
            }
            // Analyzer `_verifyConstructor`: a `const` constructor with a
            // block or expression body (real or recovery-synthetic) reports
            // at the body's opening brace / `=>`; `;` bodies stay clean.
            if let Some(c) = var_final_or_const
                && self.cursor.lexeme(c.id) == "const"
                && body_kind != TokenKind::Semicolon
                && body_kind != TokenKind::Eq
            {
                self.report(msg::const_constructor_with_body(), body_begin);
            }
            match kind {
                DeclarationKind::Mixin => self.report(msg::mixin_declares_constructor(), name),
                DeclarationKind::Extension => {
                    self.report(msg::extension_declares_constructor(), name);
                }
                _ => {}
            }
            let begin = begin_param;
            self.listener.end_constructor(
                kind,
                begin,
                new_kw,
                begin_param,
                begin_initializers,
                end,
            );
        } else {
            if get_or_set.is_some()
                && enclosing_declaration_name == Some(self.cursor.lexeme(name.id))
            {
                self.report(msg::member_with_same_name_as_class(), name);
            }
            if let Some(vfc) = var_final_or_const {
                self.report(msg::const_method(), vfc);
            }
            // error_verifier `extensionDeclaresAbstractMember` (the parse
            // body is `;` and the extension member is neither static nor
            // external): reported at the member name.
            if kind == DeclarationKind::Extension
                && static_kw.is_none()
                && mods.external.is_none()
                && body_kind == TokenKind::Semicolon
            {
                self.report(msg::extension_declares_abstract_member(), name);
            }
            self.listener.end_method(
                kind,
                get_or_set,
                begin_param,
                begin_param,
                begin_initializers,
                end,
            );
        }
    }

    /// Official `parseFactoryMethod`.
    fn parse_factory_method(
        &mut self,
        kind: DeclarationKind,
        augment: Option<TokenSpan>,
        external: Option<TokenSpan>,
        static_or_covariant: Option<TokenSpan>,
        var_final_or_const: Option<TokenSpan>,
        has_name: bool,
    ) {
        let factory = self.peek_span();
        self.cursor.advance();

        if let Some(s) = static_or_covariant {
            self.report(msg::extraneous_modifier(self.cursor.lexeme(s.id)), s);
        }
        let mut var_final_or_const = var_final_or_const;
        if let Some(vfc) = var_final_or_const
            && self.cursor.lexeme(vfc.id) != "const"
        {
            self.report(msg::extraneous_modifier(self.cursor.lexeme(vfc.id)), vfc);
            var_final_or_const = None;
        }

        let last_consumed = factory;
        self.listener
            .begin_factory(kind, last_consumed, augment, external, var_final_or_const);

        if !has_name {
            let at = self.peek_span();
            self.listener
                .handle_no_identifier(at, IdentifierContext::MethodDeclaration);
        } else if self.peek_kind() == TokenKind::Keyword(Keyword::New)
            && self.cursor.look_kind(1) != TokenKind::Period
        {
            let ident = self.peek_span();
            self.report(msg::factory_constructor_new_name(), ident);
            self.cursor.advance();
            self.listener
                .handle_identifier(ident, IdentifierContext::MethodDeclaration);
        } else {
            self.ensure_identifier(IdentifierContext::MethodDeclaration);
            self.parse_qualified_rest_opt(IdentifierContext::MethodDeclarationContinuation);
        }
        let name_end = self.peek_span();
        let _ = self.parse_method_type_var(name_end);
        self.parse_formal_parameters_required_opt(MemberKind::Factory);
        let async_kw = self.parse_async_modifier_opt();
        if self.async_state != AsyncModifier::Sync {
            self.report(msg::factory_not_sync(), async_kw);
        }
        let end;
        if self.peek_kind() == TokenKind::Eq {
            if external.is_some() {
                self.report(msg::external_factory_redirection(), self.peek_span());
            }
            end = self.parse_redirecting_factory_body();
        } else if external.is_some() {
            if self.peek_kind() != TokenKind::Semicolon {
                self.report(msg::external_factory_with_body(), self.peek_span());
            }
            end = self.parse_function_body(false, true);
        } else {
            // Official `parseFactoryMethod`: a `const` modifier with a real
            // body (the `handleConstFactory` event; external and redirecting
            // factories bypass it) is the analyzer's `const_factory`.
            if let Some(c) = var_final_or_const
                && self.cursor.lexeme(c.id) == "const"
            {
                self.report(msg::const_factory(), c);
            }
            end = self.parse_function_body(false, self.features.augmentations_enabled);
        }
        match kind {
            DeclarationKind::Mixin => self.report(msg::mixin_declares_constructor(), factory),
            DeclarationKind::Extension => {
                self.report(msg::extension_declares_constructor(), factory);
            }
            _ => {}
        }
        self.listener.end_factory(kind, last_consumed, factory, end);
    }

    /// Official `parseInvalidOperatorDeclaration` (missing/invalid `operator`
    /// keyword).
    fn parse_invalid_operator_declaration(
        &mut self,
        kind: DeclarationKind,
        mods: MemberModifiers,
        before_type: crate::cursor::CursorMark,
    ) {
        // Skip straight to the operator token (official :11224-11237: the
        // operator is the head when it `isOperator`, not only the
        // user-definable set) and parse it as a method with a synthetic
        // `operator` introducer spliced in.
        while !Self::is_operator_token(self.peek_kind())
            && !matches!(
                self.peek_kind(),
                TokenKind::EqEqEq | TokenKind::BangEqEq | TokenKind::Eof
            )
        {
            self.force_advance();
        }
        let at = self.peek_span();
        self.report(msg::missing_operator_keyword(), at);
        self.synthetic(TokenKind::Keyword(Keyword::Operator), at.range.offset);
        self.parse_method(
            kind,
            None,
            mods,
            before_type,
            TypeInfo::NoType,
            None,
            None,
            false,
        );
    }

    /// Official `recoverFromInvalidMember`.
    fn recover_from_invalid_member(
        &mut self,
        kind: DeclarationKind,
        enclosing_declaration_name: Option<&'s str>,
        mods: MemberModifiers,
        member_start: crate::cursor::CursorMark,
        before_type: crate::cursor::CursorMark,
        type_info: TypeInfo,
        get_or_set: Option<TokenSpan>,
        new_kw: Option<TokenSpan>,
    ) {
        let next = self.cursor.peek();
        let next_kind = next.kind;
        let next_span = next.span;
        match next_kind {
            TokenKind::Keyword(Keyword::Class) => {
                self.report(msg::class_in_class(), next_span);
                self.listener.handle_invalid_member(next_span);
                self.skip_member_shaped_declaration();
                return;
            }
            TokenKind::Keyword(Keyword::Enum) => {
                self.report(msg::enum_in_class(), next_span);
                self.listener.handle_invalid_member(next_span);
                self.skip_member_shaped_declaration();
                return;
            }
            TokenKind::Keyword(Keyword::Typedef) => {
                self.report(msg::typedef_in_class(), next_span);
                self.listener.handle_invalid_member(next_span);
                // Official `reportAndSkipTypedefInClass` (:11510) reports and
                // returns without skipping — the TODO above its skip loop was
                // never implemented, so the body member loop reparses from
                // the token after `typedef` and a well-formed alias shape
                // (`typedef T = int;`) rides in as a plain field parse with
                // its `missing_const_final_var_or_type` report. Consume the
                // `typedef` keyword so the reparse starts there (the official
                // loop re-enters with `token` parked on it).
                self.cursor.advance();
                return;
            }
            _ => {}
        }
        // Official :11315-11316: an operator-token head (e.g. a stray `=`
        // member) recovers as an operator declaration missing the keyword
        // (`next.isOperator && next.endGroup == null`; bracket heads carry a
        // group and stay excluded).
        if !matches!(
            next_kind,
            TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly
        ) && Self::is_operator_token(next_kind)
        {
            self.parse_invalid_operator_declaration(kind, mods, before_type);
            return;
        }
        if get_or_set.is_some()
            || matches!(
                next_kind,
                TokenKind::OpenParen | TokenKind::Function | TokenKind::OpenCurly
            )
        {
            self.parse_method(
                kind,
                enclosing_declaration_name,
                mods,
                before_type,
                type_info,
                get_or_set,
                new_kw,
                false,
            );
        } else if self.cursor.progress() == crate::cursor::TokenCursor::progress_at(member_start)
            && !matches!(next_kind, TokenKind::CloseCurly)
        {
            self.report(msg::expected_class_member(), next_span);
            self.listener.handle_invalid_member(next_span);
            self.cursor.advance();
        } else if !matches!(next_kind, TokenKind::CloseCurly | TokenKind::Eof) {
            self.parse_fields(kind, before_type, mods, before_type, type_info, None, false);
        } else {
            self.report(msg::expected_class_member(), next_span);
            self.listener.handle_invalid_member(next_span);
        }
    }

    /// `reportAndSkipClassInClass`/`reportAndSkipEnumInClass` shape: cursor
    /// is on the `class`/`enum` keyword; skip `name { body }` when it looks
    /// like a nested declaration.
    fn skip_member_shaped_declaration(&mut self) {
        self.cursor.advance(); // `class`/`enum`
        if is_identifier(self.peek_kind()) {
            self.cursor.advance();
            if self.peek_kind() == TokenKind::OpenCurly
                && let Some(close) = self.current_matching_end()
            {
                self.skip_to_token(close);
                self.cursor.advance();
            }
        }
    }

    // ------------------------------------------------------------------
    // shared member pieces
    // ------------------------------------------------------------------

    /// Official `parseFields`.
    // `type_info` is consumed by some recovery paths only; the signature
    // mirrors the official `parseFields`.
    #[allow(clippy::needless_pass_by_value)]
    fn parse_fields(
        &mut self,
        kind: DeclarationKind,
        _before_start: crate::cursor::CursorMark,
        mods: MemberModifiers,
        before_type: crate::cursor::CursorMark,
        type_info: TypeInfo,
        synthetic_name: Option<TokenSpan>,
        name_is_recovered: bool,
    ) {
        self.listener.begin_fields(
            kind,
            mods.augment,
            mods.abstract_kw,
            mods.external,
            mods.static_kw,
            mods.covariant,
            mods.late_kw,
            mods.var_final_or_const,
            self.span_at_mark(before_type),
        );

        if let Some(c) = mods.covariant
            && mods.late_kw.is_none()
            && let Some(vfc) = mods.var_final_or_const
            && self.cursor.lexeme(vfc.id) == "final"
        {
            self.report(msg::final_and_covariant(), c);
        }
        if matches!(type_info, TypeInfo::NoType) {
            if mods.var_final_or_const.is_none() {
                // The face anchors on the type-position token (official
                // parseFields reports on the member's head — probed 3.13.1:
                // `late;` → the `late` keyword, `late int;`/`int;`/`x;` → the
                // type-position identifier). A junk token at the type position
                // (`late;`'s `;`) falls back to the `late` keyword; a name-
                // position identifier (`late x;`) IS the type-position token,
                // which is also the official `name` anchor — the face fires
                // unconditionally for `noType` + no `var`/`final`/`const`,
                // including `late x;` fields at top-level and member depth
                // (official parseFields has no late carve-out;
                // parser_impl.dart:4476-4479, intertwined trace reports on
                // x/y/z at every depth in late_without_var_etc).
                // In the late-free shapes (`int;`/`x;` → the type-position
                // identifier) the anchor stays on that identifier.
                let at_type = self.span_at_mark(before_type);
                let type_kind =
                    crate::cursor::TokenCursor::mark_pos(before_type).min(self.table.len() - 1);
                let at = if is_keyword_or_identifier(self.kind_at(type_kind)) {
                    at_type
                } else {
                    mods.late_kw.unwrap_or(at_type)
                };
                self.report(msg::missing_const_final_var_or_type(), at);
            }
        } else if let Some(vfc) = mods.var_final_or_const
            && self.cursor.lexeme(vfc.id) == "var"
        {
            self.report(msg::type_after_var(), vfc);
        }
        if let (Some(a), true) = (mods.abstract_kw, mods.external.is_some()) {
            self.report(msg::abstract_external_field(), a);
        }
        // Official `_endClassFields` modifier validation (class-like fields;
        // extension fields have their dedicated codes below).
        if !matches!(
            kind,
            DeclarationKind::Extension | DeclarationKind::ExtensionType
        ) {
            if let Some(a) = mods.abstract_kw {
                if mods.static_kw.is_some() {
                    self.report(msg::abstract_static_field(), a);
                }
                if mods.late_kw.is_some() {
                    self.report(msg::abstract_late_field(), a);
                }
            }
            if let Some(e) = mods.external
                && mods.late_kw.is_some()
            {
                self.report(msg::external_late_field(), e);
            }
        }

        // The synthetic name's queued token is truncated by the reset below
        // (its insert predates the `before_type` mark), so snapshot its
        // lexeme — empty for a synthetic identifier — beforehand; the id
        // stays span-valid but must not be lexeme-read afterwards.
        let synthetic_name_text = synthetic_name.map(|s| self.cursor.lexeme(s.id));
        self.cursor.reset(before_type);
        type_info.parse_type(self);

        let context = match kind {
            DeclarationKind::TopLevel => IdentifierContext::TopLevelVariableDeclaration,
            _ => IdentifierContext::FieldDeclaration,
        };
        let first_name = match synthetic_name {
            Some(name) => name,
            None if mods.var_final_or_const.is_none()
                && !is_keyword_or_identifier(self.peek_kind()) =>
            {
                // Member-level junk-name recovery arriving without a
                // `var`/`final`/`const` modifier (`late ;`, `int ;`,
                // `int }` — the class-member `recoverFromInvalidMember`
                // fallback): the official insert is silent here and
                // `parseFields`' type face is the only row (probed oracle
                // 3.13.1: `late ;` → missingConstFinalVarOrType on `late`;
                // `var ;` → missing_identifier on `;`). The keyword case
                // stays on `ensureIdentifier`, which owns its own
                // reporting split.
                let at = self.peek_span();
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Identifier, at.range.offset);
                self.cursor.advance();
                self.span_of(id)
            }
            None => {
                if name_is_recovered {
                    self.ensure_identifier_recovered(context)
                } else {
                    self.ensure_identifier(context)
                }
            }
        };

        if let Some(c) = mods.covariant
            && mods.late_kw.is_some()
            && let Some(vfc) = mods.var_final_or_const
            && self.cursor.lexeme(vfc.id) == "final"
            && self.peek_kind() == TokenKind::Eq
        {
            self.report(msg::final_and_covariant_late_with_initializer(), c);
        }

        // Official `VariableDeclarationList` head (`_endClassFields`,
        // ast_builder.dart): `late` when present, else the var/final/const
        // keyword, else the type start — `late` must be the span head
        //.
        let decl_head = mods
            .late_kw
            .or(mods.var_final_or_const)
            .unwrap_or_else(|| self.span_at_mark(before_type));
        self.listener
            .begin_variables_declaration(decl_head, mods.late_kw, mods.var_final_or_const);
        let first_name_text = match synthetic_name_text {
            Some(text) => text,
            None => self.cursor.lexeme(first_name.id),
        };
        let mut field_count = 1usize;
        self.parse_field_initializer_opt(first_name, first_name_text, kind, &mods);
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            let name = self.ensure_identifier(context);
            let name_text = self.cursor.lexeme(name.id);
            self.parse_field_initializer_opt(name, name_text, kind, &mods);
            field_count += 1;
        }
        let had_semicolon = self.peek_kind() == TokenKind::Semicolon;
        let end = self.ensure_semicolon();
        // The missing-`;` field-group recovery re-enters the member loop at
        // the follower, and the group's written name re-enters the next
        // member as its annotation (the `augment int field;` recovery — the
        // member splits at the name `int`, and the reparsed `int` type
        // reference is the lookup the member-shadow rule resolves; 3.13.1
        // oracle, member_declarations.dart line 14). Fire the NamedType
        // event for the written name so the reparsed reference reaches
        // resolution.
        if !had_semicolon && self.peek_kind() == TokenKind::Identifier && synthetic_name.is_none() {
            self.listener.handle_type(first_name, None);
        }
        self.listener
            .end_variables_declaration(field_count, Some(end));

        match kind {
            DeclarationKind::Extension => {
                if let Some(a) = mods.abstract_kw {
                    self.report(msg::abstract_extension_field(), a);
                }
                if mods.static_kw.is_none() && mods.external.is_none() {
                    self.report(msg::extension_declares_instance_field(), first_name);
                }
            }
            DeclarationKind::ExtensionType
                if mods.static_kw.is_none() && mods.external.is_none() =>
            {
                self.report(msg::extension_type_declares_instance_field(), first_name);
            }
            _ => {}
        }

        let begin = self.peek_span_at_mark(before_type);
        if kind == DeclarationKind::TopLevel {
            self.listener.end_top_level_fields(
                mods.augment,
                mods.abstract_kw,
                mods.external,
                mods.static_kw,
                mods.covariant,
                mods.late_kw,
                mods.var_final_or_const,
                field_count,
                begin,
                end,
            );
        } else {
            self.listener.end_fields(
                kind,
                mods.abstract_kw,
                mods.augment,
                mods.external,
                mods.static_kw,
                mods.covariant,
                mods.late_kw,
                mods.var_final_or_const,
                field_count,
                begin,
                end,
            );
        }
    }

    /// Official `parseFieldInitializerOpt`.
    ///
    /// The official `ast_builder` folds the field-initializer events into
    /// `VariableDeclaration(equals, initializer)` (`ast_builder.dart:1749`)
    /// — the `VariablesDeclaration > InitializedIdentifier` shape. No
    /// `FieldInitializer` node exists for fields; that node is the
    /// constructor `this.x = ...` initializer only (`ast_builder.dart:729`).
    fn parse_field_initializer_opt(
        &mut self,
        name: TokenSpan,
        name_text: &'s str,
        kind: DeclarationKind,
        mods: &MemberModifiers,
    ) {
        self.listener.begin_initialized_identifier(name);
        if self.peek_kind() == TokenKind::Eq {
            self.parse_variable_initializer_opt();
        } else {
            if let Some(vfc) = mods.var_final_or_const {
                let lexeme = self.cursor.lexeme(vfc.id);
                if lexeme == "const" {
                    self.report(msg::const_not_initialized(name_text), name);
                } else if lexeme == "final"
                    && kind == DeclarationKind::TopLevel
                    && mods.late_kw.is_none()
                    && mods.abstract_kw.is_none()
                    && mods.external.is_none()
                {
                    self.report(msg::final_not_initialized(name_text), name);
                }
            }
            let next = self.peek_span();
            self.listener.handle_no_variable_initializer(next);
        }
        self.listener.end_initialized_identifier(name);
    }

    /// Official `parseMethodTypeVar` (`<...>` after a method name).
    /// Official `parseMethodTypeVar`; returns the type-parameter list's
    /// closer span when type variables were present (`Some`), `None`
    /// otherwise.
    fn parse_method_type_var(&mut self, name: TokenSpan) -> Option<TokenSpan> {
        if self.peek_kind() == TokenKind::Bang {
            let at = self.peek_span();
            self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
            self.cursor.advance();
        }
        if self.peek_kind() != TokenKind::Lt {
            let _ = name;
            return None;
        }
        let lt = self.peek_span();
        let type_param = self.compute_type_param_or_arg_on_lt(true);
        let mut closer = lt;
        if !matches!(type_param, TypeParamOrArg::None) {
            type_param.parse_variables(self, false);
            closer = self.last_consumed_span();
        }
        if self.peek_kind() == TokenKind::Eq {
            let at = self.peek_span();
            self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
            self.cursor.advance();
        }
        let _ = name;
        Some(closer)
    }

    /// Official `parseQualifiedRestOpt`: `. identifier` suffix.
    pub(crate) fn parse_qualified_rest_opt(&mut self, context: IdentifierContext) {
        if self.peek_kind() != TokenKind::Period {
            return;
        }
        let period = self.peek_span();
        self.cursor.advance();
        self.ensure_identifier(context);
        self.listener.handle_qualified(period);
    }

    /// Official `parseGetterOrFormalParameters`.
    fn parse_getter_or_formal_parameters(
        &mut self,
        name: TokenSpan,
        is_getter: bool,
        kind: MemberKind,
    ) -> TokenSpan {
        self.synthetic_formals = false;
        if self.peek_kind() == TokenKind::OpenParen {
            if is_getter {
                self.report(msg::getter_with_formals(), self.peek_span());
            }
            self.parse_formal_parameters_rest_full(kind)
        } else if is_getter {
            let next = self.peek_span();
            self.listener.handle_no_formal_parameters(next, kind);
            next
        } else {
            // Recovery: report and parse a synthetic empty parameter list.
            // The official `insertSyntheticToken` pair splices into the
            // stream and the parser moves past it (`parseGetterOrFormalParameters`
            // recovery, :5560); drain the queue so the body-section reads
            // see the real follower, not the queued synthetics. The group
            // counts as empty (not a stale predecessor's count) and carries
            // the synthetic-`(` marker the official setter check keys on
            // (`ast_builder.dart:6475`).
            // Official `reportRecoverableError` anchor walk
            // (`findNonZeroLengthToken`, parser/util.dart:56): a zero-width
            // synthetic name — the class-body recovery's inserted identifier
            // parked at the real follower — anchors on the first real
            // follower token (manual face factory_with_initializers/1: the
            // oracle's `missing_method_parameters` spans the `{`, 3:23|1).
            let mut anchor = name;
            let mut n = 0usize;
            while anchor.range.length == 0 && self.cursor.look_kind(n) != TokenKind::Eof {
                n += 1;
                anchor = self.look_span_at(n);
            }
            self.report(msg::missing_parameters(kind), anchor);
            self.last_parameter_count = 0;
            self.synthetic_formals = true;
            let at = self.peek_span();
            let open = self.synthetic(TokenKind::OpenParen, at.range.offset);
            let close = self.synthetic(TokenKind::CloseParen, at.range.offset);
            self.listener.begin_formal_parameters(open, kind);
            self.listener.end_formal_parameters(0, open, close, kind);
            self.cursor.advance();
            self.cursor.advance();
            close
        }
    }

    /// Full parameter entry: `(` present, else no-parameters event.
    fn parse_formal_parameters_rest_full(&mut self, kind: MemberKind) -> TokenSpan {
        if self.peek_kind() == TokenKind::OpenParen {
            self.parse_formal_parameters_rest(kind)
        } else {
            let next = self.peek_span();
            self.listener.handle_no_formal_parameters(next, kind);
            next
        }
    }

    /// Official `parseInitializersOpt` (constructor `: init, init` lists).
    pub(crate) fn parse_initializers_opt(&mut self) -> Option<TokenSpan> {
        if self.peek_kind() != TokenKind::Colon {
            self.listener.handle_no_initializers();
            return None;
        }
        let begin = self.peek_span();
        self.listener.begin_initializers(begin);
        self.cursor.advance();
        // `mayParseFunctionExpressions = false` (:4756): `(a) {}` in an
        // initializer is not a function literal.
        let saved = self.may_parse_function_expressions;
        self.may_parse_function_expressions = false;
        let mut count = 0usize;
        let mut last_end = begin;
        let mut last_real = self.cursor.real_pos();
        let mut turns = 0usize;
        loop {
            if self.is_bailed() {
                // Terminal bailout: unwind without consumption (see
                // `parse_list_literal` in literals.rs).
                break;
            }
            turns += 1;
            if turns > INITIALIZER_TURN_CAP {
                // Belt over the no-progress guard below: that guard breaks
                // only when the real stream is parked with the queue
                // drained; a parked stream whose queue is re-filled as fast
                // as each turn drains it never satisfies it, and the
                // repeated same-anchor reports dedupe away (zero diagnostic
                // burn), so no other cap would ever stop the loop. The cap
                // is unreachable by valid input — see
                // [`INITIALIZER_TURN_CAP`] — and latches the same one-shot
                // terminal bailout as the other caps.
                self.bail(crate::message::Message::new(
                    dartq_diagnostics::DARTQ_INTERNAL_ERROR,
                    format!(
                        "Constructor initializer recovery exceeded \
                         {INITIALIZER_TURN_CAP} turns; the rest of the file \
                         was not analyzed."
                    ),
                ));
                break;
            }
            last_end = self.parse_initializer(last_end);
            count += 1;
            if self.peek_kind() != TokenKind::Comma {
                // Recovery (:4770-4798): the head looks like another entry
                // (`assert(`, `this`/`super` + `(`/`.`, `name =`) — a comma
                // went missing; report and insert one.
                if self.initializer_head_continues() {
                    // No-progress guard (the structural-termination
                    // invariant): an entry that leaves the real stream
                    // parked with an empty queue would re-parse the same
                    // head forever — the `super.foo;` recovery's synthetic
                    // parens land AHEAD of the `super` head, so the
                    // expression re-read returns without consuming it, and
                    // each turn re-reported and re-queued its tokens
                    // (unbounded diagnostics, GBs of RSS). The member loop
                    // owns the stall from here.
                    let real = self.cursor.real_pos();
                    if real == last_real && self.cursor.progress() == real as u64 * 2 {
                        break;
                    }
                    last_real = real;
                    self.report(msg::expected_after_but_got(","), last_end);
                    let at = self.peek_span().range.offset;
                    self.synthetic(TokenKind::Comma, at);
                    self.cursor.advance();
                    continue;
                }
                break;
            }
            last_real = self.cursor.real_pos();
            self.cursor.advance();
        }
        self.may_parse_function_expressions = saved;
        self.listener.end_initializers(count, begin, last_end);
        Some(begin)
    }

    /// Whether the token at the cursor starts another initializer entry
    /// (the missing-comma lookahead of [`Self::parse_initializers_opt`]).
    fn initializer_head_continues(&self) -> bool {
        match self.peek_kind() {
            TokenKind::Keyword(Keyword::Assert) => self.cursor.look_kind(1) == TokenKind::OpenParen,
            TokenKind::Keyword(Keyword::This | Keyword::Super) => matches!(
                self.cursor.look_kind(1),
                TokenKind::OpenParen | TokenKind::Period
            ),
            kind if is_identifier(kind) => self.cursor.look_kind(1) == TokenKind::Eq,
            _ => false,
        }
    }

    /// Official `parseInitializer` (:4815): `super (...|.name ...)`,
    /// `assert(...)`, `this.name = ...`/`this(...)`, `name = expr`, with the
    /// official synthetic-assignment recoveries. `previous` is the token
    /// before the entry head (the official `token`/`beforeExpression`; the
    /// report anchors and the expression re-read start there).
    fn parse_initializer(&mut self, previous: TokenSpan) -> TokenSpan {
        let head = self.peek_span();
        self.listener.begin_initializer(head);
        if self.peek_kind() == TokenKind::Keyword(Keyword::Assert) {
            let end = self.parse_assert(AssertKind::Initializer);
            self.listener.end_initializer(end);
            return end;
        }
        if self.peek_kind() == TokenKind::Keyword(Keyword::Super) {
            return self.parse_super_initializer_expression();
        }
        if self.peek_kind() == TokenKind::Keyword(Keyword::This) {
            let next = self.cursor.look_kind(1);
            if next == TokenKind::Period {
                // `this.name = ...`: lookahead only — the expression re-read
                // reparses the `this.name` head natively.
                let name_kind = self.cursor.look_kind(2);
                let after_name = self.cursor.look_kind(3);
                // `_tryRewriteNewToIdentifier` (:4830-4834): for
                // `this.new(` the reserved `new` is rewritten to the name
                // before the identifier check — no report.
                let rewritten = name_kind == TokenKind::Keyword(Keyword::New)
                    && after_name == TokenKind::OpenParen;
                if !rewritten && !is_identifier(name_kind) {
                    // Official `insertSyntheticIdentifier(dot,
                    // fieldInitializer)` (:4841); the follow-up Eq/OpenParen
                    // checks then read the *real* name token (:4845-4858) —
                    // e.g. `new` is neither `=` nor `(` — so the entry falls
                    // to the final synthetic-assignment recovery (:4902).
                    // The official stream is `<id2> = this . <id1> <name>`;
                    // the queued tokens here would drain ahead of the real
                    // stream, so the `this . <id1>` prefix is driven
                    // directly and the entry ends on the real keyword (the
                    // remaining `= expr` then re-enters the member loop as
                    // the official cascade does).
                    let at = self.look_span_at(2);
                    // The slot template is `expectedIdentifier`
                    // (fieldInitializer recoveryTemplate,
                    // identifier_context.dart:305), whose pseudo-shared-code
                    // conversion re-renders the parameterless registry text
                    // (frozen oracle ccf7f50c/d1b3378f `this.new` slots).
                    self.report(msg::missing_identifier(), at);
                    self.report(msg::missing_assignment_in_initializer(), head);
                    self.cursor.advance(); // `this`
                    self.cursor.advance(); // `.`
                    // The synthetic name carries exactly ONE identifier
                    // event — the official re-read's
                    // `ensureIdentifier(., expressionContinuation)`
                    // (`handleIdentifier(, expressionContinuation)`,
                    // new_as_identifier trace). No `new` rewrite here: the
                    // rewrite face is gated on the `(` follower
                    // (:4830-4834), so the reserved word never reaches an
                    // identifier gate — it is left for the member loop.
                    let name = self.synthetic(TokenKind::Identifier, at.range.offset);
                    self.cursor.advance(); // the synthetic name
                    self.listener
                        .handle_identifier(name, IdentifierContext::ExpressionContinuation);
                    self.listener.end_initializer(name);
                    return name;
                }
                let follower = after_name;
                if follower == TokenKind::Eq {
                    return self.parse_initializer_expression_rest();
                }
                if follower == TokenKind::OpenParen {
                    return self.finish_redirecting_initializer();
                }
                // Fall through to the synthetic-assignment recovery, with
                // `this.name` (or the synthetic name) left unconsumed.
                self.report(msg::missing_assignment_in_initializer(), head);
                self.synthetic(TokenKind::Identifier, head.range.offset);
                self.synthetic(TokenKind::Eq, head.range.offset);
                return self.parse_initializer_expression_rest();
            }
            if next == TokenKind::OpenParen {
                return self.finish_redirecting_initializer();
            }
            // Recovery (:4873-4881): `expectedButGot('.')`, then the official
            // queues a synthetic `.` + name *after* `this` and falls through
            // to the synthetic assignment (re-read `<name> = this . <name>`).
            // The queue splices at a single point, so the pair cannot land
            // between `this` and the follower while the head re-read starts
            // before `this`; we emit the same two reports and parse
            // `<name> = this`, leaving the follower to the initializer loop
            // at the same break point as the oracle.
            let at = self.look_span_at(1);
            self.report(msg::expected_but_got("."), at);
            self.report(msg::missing_assignment_in_initializer(), head);
            self.synthetic(TokenKind::Identifier, head.range.offset);
            self.synthetic(TokenKind::Eq, head.range.offset);
            return self.parse_initializer_expression_rest();
        }
        let head_kind = self.peek_kind();
        if is_identifier(head_kind) {
            if self.cursor.look_kind(1) == TokenKind::Eq {
                return self.parse_initializer_expression_rest();
            }
            // Recovery (:4890-4903): a non-operator follower means the name
            // is the assignment target — the official stream is
            // `x = <synthetic>(3)` (`rewriter.insertSyntheticToken(next, EQ)`
            // + `insertSyntheticIdentifier`, issue_31192), so the head is the
            // assignment LHS and the queued synthetic pair is the RHS. The
            // queue splices at the cursor (it cannot land after the head), so
            // the official event stream is driven directly: head send, the
            // inserted `=`, the synthetic primary, the assignment event.
            let follower = self.cursor.look_kind(1);
            if crate::expressions::kind_precedence(follower).is_none() {
                self.report(msg::missing_assignment_in_initializer(), head);
                let at = self.look_span_at(1);
                // LHS: the bare name with its no-arguments send face (the
                // follower `(` belongs to the RHS, not to the target).
                let lhs = self.ensure_identifier(IdentifierContext::Expression);
                self.listener.handle_no_type_arguments(at);
                self.listener.handle_no_arguments(at);
                self.listener.handle_send(lhs, lhs);
                let equals = self.synthetic(TokenKind::Eq, at.range.offset);
                self.cursor.advance(); // the inserted `=`
                let name = self.synthetic(TokenKind::Identifier, at.range.offset);
                self.cursor.advance(); // the synthetic RHS primary
                let end = self.parse_send_from_identifier(name);
                self.listener.handle_assignment_expression(equals, end);
                self.listener.end_initializer(end);
                return end;
            }
            // Operator/`.` follower: fall through to the synthetic-assignment
            // recovery with the head untouched.
        } else {
            // Recovery (:4905-4915): no entry head at all — insert a
            // synthetic assignment `<name> = <name>`; the report anchors on
            // the token before the entry (official `messageOnToken: token`).
            self.report(msg::expected_an_initializer(), previous);
            let at = head.range.offset;
            self.synthetic(TokenKind::Identifier, at);
            self.synthetic(TokenKind::Eq, at);
            self.synthetic(TokenKind::Identifier, at);
            return self.parse_initializer_expression_rest();
        }
        self.report(msg::missing_assignment_in_initializer(), head);
        self.synthetic(TokenKind::Identifier, head.range.offset);
        self.synthetic(TokenKind::Eq, head.range.offset);
        self.parse_initializer_expression_rest()
    }

    /// Parses `this(...)`/`this.name(...)` as a redirecting initializer and
    /// checks the body that follows (:4860-4866).
    fn finish_redirecting_initializer(&mut self) -> TokenSpan {
        // AstBuilder initializer recovery (`ast_builder.dart:803-815`):
        // `this(...)` followed by a selector (`this().foo`) reports
        // `invalid_this_in_initializer` at `this`.
        let mut n = 1;
        if self.cursor.look_kind(n) == TokenKind::Period {
            n += 1;
        }
        if self.cursor.look_kind(n) == TokenKind::OpenParen
            && let Some(close) = self.cursor.table_matching_end(self.look_span_at(n).id)
            && matches!(
                self.kind_at(self.offset_of(close) + 1),
                TokenKind::Period | TokenKind::QuestionPeriod | TokenKind::OpenSquare
            )
        {
            self.report(msg::invalid_this_in_initializer(), self.peek_span());
        }
        let end = self.parse_initializer_expression_rest();
        let after = self.peek_kind();
        if after == TokenKind::OpenCurly || after == TokenKind::Function {
            self.report(msg::redirecting_constructor_with_body(), self.peek_span());
        }
        end
    }

    /// Official `parseSuperInitializerExpression` (:4918): lookahead over
    /// `super (. name)?` to validate the shape — the `?.` recovery is
    /// sequential, the `=`/`(` checks re-running after the member name
    /// (:4934-4953) — then the expression re-read.
    fn parse_super_initializer_expression(&mut self) -> TokenSpan {
        let head = self.peek_span();
        let mut n = 1;
        if self.cursor.look_kind(n) == TokenKind::Period {
            n += 1;
            let name = self.cursor.look_kind(n);
            if !(is_identifier(name) || name == TokenKind::Keyword(Keyword::New)) {
                // Official `expressionContinuation.ensureIdentifier`.
                let at = self.look_span_at(n);
                self.report(msg::missing_identifier(), at);
                self.synthetic(TokenKind::Identifier, at.range.offset);
            }
            n += 1;
        }
        let next = self.cursor.look_kind(n);
        if next != TokenKind::OpenParen {
            if next == TokenKind::QuestionPeriod {
                // `super?.` (:4936-4953) — the official consumes `?.`,
                // splices a missing member name silently, re-runs the
                // `=`/`(` checks on the follower, then re-reads the
                // initializer as an expression. The cursor splices
                // synthetics at the cursor only (never mid-stream), so the
                // re-read's head is driven here with the official event
                // shape — `handleSuperExpression`, the `?.`-on-super
                // report itself (`parseSuperExpression` :7698), the
                // null-aware selector pair — and the member send runs
                // through the real send machinery.
                self.cursor.advance(); // `super`
                self.listener
                    .handle_super_expression(head, IdentifierContext::Expression);
                let qdot = self.peek_span();
                self.report(msg::invalid_operator_questionmark_period_for_super(), qdot);
                self.cursor.advance(); // `?.`
                self.listener.handle_null_aware_member_begin(qdot);
                if !(is_identifier(self.peek_kind())
                    || self.peek_kind() == TokenKind::Keyword(Keyword::New))
                {
                    // Insert a synthetic identifier but don't report
                    // another error (:4941-4943).
                    let at = self.peek_span();
                    self.synthetic(TokenKind::Identifier, at.range.offset);
                }
                // The member-name head: `parse_send`'s first half (the
                // identifier event), with `new` read as the name
                // (`parseSendContinuation`'s head arm).
                let name = if self.peek_kind() == TokenKind::Keyword(Keyword::New) {
                    let new_kw = self.peek_span();
                    self.cursor.advance();
                    self.listener
                        .handle_identifier(new_kw, IdentifierContext::ExpressionContinuation);
                    new_kw
                } else {
                    self.ensure_identifier(IdentifierContext::ExpressionContinuation)
                };
                // The `=`/`(` checks re-run on the follower after the name
                // (:4945-4953) — the official `?.` branch is sequential,
                // not an alternative. The `=` rider anchors on the member
                // name (the official's `token`); the `(`-expect wraps the
                // name in empty parens spliced right at the cursor
                // (`insertParens(includeIdentifier: false)`), where the
                // send tail then reads them as the argument group.
                match self.peek_kind() {
                    TokenKind::Eq => {
                        self.report(msg::field_initialized_outside_declaring_class(), name);
                    }
                    TokenKind::OpenParen => {}
                    _ => {
                        let at = self.peek_span();
                        self.report(msg::expected_after_but_got("("), at);
                        self.synthetic(TokenKind::OpenParen, at.range.offset);
                        self.synthetic(TokenKind::CloseParen, at.range.offset);
                    }
                }
                let operand = self.parse_send_from_identifier(name);
                self.listener.handle_dot_access(qdot, operand, true);
                self.listener.end_initializer(operand);
                return operand;
            }
            if next == TokenKind::Eq {
                let before_eq = self.cursor.look_kind(n - 1);
                if before_eq == TokenKind::Keyword(Keyword::Super) {
                    // Bare `super = ...`: the parser stays silent ("parseExpression
                    // will report error on assignment to super") but the analyzer
                    // builder cannot build an initializer from a bare super
                    // assignment (`buildInitializer` `default: return null`) and
                    // reports `invalidInitializer` at the initializer begin.
                    self.report(msg::invalid_initializer(), head);
                } else {
                    // `super.name = ...` — parseExpression reports the
                    // assignment itself; this is the declaring-class rider.
                    self.report(
                        msg::field_initialized_outside_declaring_class(),
                        self.look_span_at(n - 1),
                    );
                }
            } else if next != TokenKind::OpenParen {
                // `expectedAfterButGot('(')` + `insertParens(includeIdentifier:
                // false)` (:4948-4952): wrap the follower in empty parens.
                let at = self.look_span_at(n);
                self.report(msg::expected_after_but_got("("), at);
                let type_param = self.compute_type_param_or_arg_at(n, false);
                if matches!(type_param, TypeParamOrArg::None) {
                    self.synthetic(TokenKind::OpenParen, at.range.offset);
                    self.synthetic(TokenKind::CloseParen, at.range.offset);
                } else {
                    // `super<...>` (e.g. `: super<int>();`): the official
                    // re-read parses the initializer as a plain expression —
                    // `super` opens a relational chain whose node
                    // `buildInitializer` rejects, so `endInitializers`
                    // reports `invalid_initializer` over the expression span
                    // (ast_builder.dart:2310-2323). Probes: `super<int>();`
                    // yields exactly the `(` expect at `<` plus one
                    // `invalid_initializer` of len 12 (`super<int>()`);
                    // `super<int>;` the same pair ending at `>`. Consume the
                    // balanced `<...>` group and a following balanced `(...)`
                    // argument group, report once over the span, and leave
                    // the follower (`;`) to the member loop — the synthetic
                    // parens must not shadow the type group here or the
                    // member-recovery cascade fires.
                    for _ in 0..n {
                        self.cursor.advance(); // `super` (. name)? / `?.` name
                    }
                    let mut end_id = type_param.skip(self.peek_id());
                    let after = self.offset_of(end_id) + 1;
                    if self.kind_at(after) == TokenKind::OpenParen
                        && let Some(close) = self.cursor.table_matching_end(self.real_id(after))
                    {
                        end_id = close;
                    }
                    let end_span = self.span_of(end_id);
                    self.report(
                        msg::invalid_initializer(),
                        TokenSpan::new(
                            head.id,
                            SourceRange {
                                offset: head.range.offset,
                                length: end_span.range.offset + end_span.range.length
                                    - head.range.offset,
                            },
                        ),
                    );
                    self.skip_to_token(end_id);
                    self.cursor.advance();
                    self.listener.end_initializer(end_span);
                    return end_span;
                }
            }
        } else if let Some(close) = self.cursor.table_matching_end(self.look_span_at(n).id) {
            let after = self.kind_at(self.offset_of(close) + 1);
            if matches!(
                after,
                TokenKind::Period | TokenKind::QuestionPeriod | TokenKind::OpenSquare
            ) {
                // `super(...)` followed by a selector (`super().foo`): AstBuilder
                // initializer recovery reports `invalid_super_in_initializer` at
                // `super` (ast_builder.dart:786-800).
                self.report(msg::invalid_super_in_initializer(), head);
            } else if after == TokenKind::Bang {
                // `super()!`: the postfix `!` leaves no representable
                // initializer node (`buildInitializer` final `return null`) —
                // `invalidInitializer` at the initializer begin (issue_41597).
                self.report(msg::invalid_initializer(), head);
            }
        }
        self.parse_initializer_expression_rest()
    }

    /// Official `parseInitializerExpressionRest` (:4957): the entry
    /// expression through the full engine, then `endInitializer`.
    fn parse_initializer_expression_rest(&mut self) -> TokenSpan {
        let end = self.parse_expression();
        self.listener.end_initializer(end);
        end
    }

    /// Official `parseAsyncModifierOpt`; updates the [`AsyncModifier`] state
    /// and returns the `async`/`sync` keyword token — the official
    /// `asyncToken` anchor the caller-side setter/factory not-sync reports
    /// pin (`parser_impl.dart:4644/:6060/:6230`; measured b02/c01: the row
    /// spans the keyword, never the star).
    pub(crate) fn parse_async_modifier_opt(&mut self) -> TokenSpan {
        let mut async_kw: Option<TokenSpan> = None;
        let mut star: Option<TokenSpan> = None;
        self.async_state = AsyncModifier::Sync;
        let without_modifier = self.peek_span();
        match self.peek_kind() {
            TokenKind::Keyword(Keyword::Async) => {
                async_kw = Some(self.peek_span());
                self.cursor.advance();
                if self.peek_kind() == TokenKind::Star {
                    let s = self.peek_span();
                    star = Some(s);
                    self.cursor.advance();
                    self.async_state = AsyncModifier::AsyncStar;
                } else {
                    self.async_state = AsyncModifier::Async;
                }
            }
            TokenKind::Keyword(Keyword::Sync) => {
                async_kw = Some(self.peek_span());
                self.cursor.advance();
                if self.peek_kind() == TokenKind::Star {
                    let s = self.peek_span();
                    star = Some(s);
                    self.cursor.advance();
                    self.async_state = AsyncModifier::SyncStar;
                } else {
                    self.report(msg::invalid_sync_modifier(), async_kw.unwrap());
                }
            }
            _ => {}
        }
        self.listener.handle_async_modifier(async_kw, star);
        async_kw.unwrap_or(without_modifier)
    }

    /// Official `parsePrimaryConstructorBody` (`this : init, ... { ... }`
    /// members; primary constructors are on at 3.15).
    fn parse_primary_constructor_body(&mut self) {
        let this = self.peek_span();
        self.listener.begin_primary_constructor_body(this, None);
        self.parse_initializers_opt();
        if matches!(
            self.peek_kind(),
            TokenKind::Keyword(Keyword::Async | Keyword::Sync)
        ) {
            self.report(msg::unexpected_token(self.cursor.lexeme(this.id)), this);
        }
        self.parse_async_modifier_opt();
        self.parse_function_body(false, self.async_state == AsyncModifier::Sync);
        self.listener
            .end_primary_constructor_body(this, this, None, self.peek_span());
    }

    /// Official `parseRedirectingFactoryBody` (`= C<T>.name(args);`).
    fn parse_redirecting_factory_body(&mut self) -> TokenSpan {
        let equals = self.peek_span();
        self.cursor.advance();
        self.listener.begin_redirecting_factory_body(equals);
        self.parse_constructor_reference(ConstructorReferenceContext::RedirectingFactory);
        let end = self.ensure_semicolon();
        self.listener.end_redirecting_factory_body(equals, end);
        end
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    /// `isUserDefinableOperator` on a kind.
    fn is_user_definable_operator(kind: TokenKind) -> bool {
        is_user_definable_operator_kind(kind)
    }

    /// `token.isOperator` (any operator token).
    fn is_operator_token(kind: TokenKind) -> bool {
        Self::is_user_definable_operator(kind)
            || matches!(
                kind,
                TokenKind::Eq | TokenKind::Bang | TokenKind::Comma | TokenKind::Question
            )
    }

    pub(crate) fn peek_is_reserved_at(&self, n: usize) -> bool {
        matches!(self.cursor.look_kind(n), TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Reserved)
    }

    /// `indicatesMethodOrField` at cursor-relative offset `n` (queue-aware).
    fn indicates_method_or_field_at(&self, n: usize) -> bool {
        matches!(
            self.cursor.look_kind(n),
            TokenKind::Semicolon
                | TokenKind::Eq
                | TokenKind::OpenParen
                | TokenKind::OpenCurly
                | TokenKind::Function
                | TokenKind::Lt
        )
    }

    /// `_isConstructor` (`parser_impl.dart:5734`).
    fn is_constructor(
        &self,
        name: TokenSpan,
        get_or_set: Option<TokenSpan>,
        new_kw: Option<TokenSpan>,
        enclosing_declaration_name: Option<&'s str>,
        is_operator: bool,
    ) -> bool {
        if new_kw.is_some() {
            return true;
        }
        // Official `_isConstructor` navigates from `name.next!`. In the
        // operator case the cursor already consumed `operator <op>`, so the
        // current peek is `name.next`; otherwise the cursor sits on the name.
        // The queue is empty at this decision point (the peek is a real
        // token), so `look_kind(1)` and `name.id + 1` agree with the
        // official `name.next!` walk in both cases.
        let (after, at) = if is_operator {
            (self.cursor.look_kind(0), 0usize)
        } else {
            (self.cursor.look_kind(1), 1usize)
        };
        if after == TokenKind::Period {
            return true;
        }
        let mut after = after;
        if is_operator {
            if Self::is_operator_token(after) {
                // skip the operator token itself
            } else if self.cursor.lexeme(name.id) == "unary" && after == TokenKind::Minus {
                return self.cursor.look_kind(1) == TokenKind::Period;
            }
        }
        if after == TokenKind::Bang {
            after = self.cursor.look_kind(at + 1);
        }
        if after == TokenKind::Lt {
            // Skip a `<...>` group when it is scanner-paired.
            if let Some(end) = self.cursor.table_matching_end(name.id + 1) {
                let after_group = self.kind_at(self.offset_of(end) + 1);
                if after_group == TokenKind::Eq {
                    after = TokenKind::Eq;
                } else {
                    after = after_group;
                }
            }
        }
        if after == TokenKind::OpenParen {
            if let Some(close) = self.cursor.table_matching_end(name.id + 1)
                && self.kind_at(self.offset_of(close) + 1) == TokenKind::Colon
            {
                return true;
            }
        } else if after == TokenKind::Colon {
            return true;
        }
        if get_or_set.is_some() {
            return false;
        }
        enclosing_declaration_name == Some(self.cursor.lexeme(name.id))
    }

    /// Span of the token at a saved mark.
    // `min` bounds the offset by the table length; token tables are
    // scanner-bounded far below `u32::MAX`.
    #[allow(clippy::cast_possible_truncation)]
    fn span_at_mark(&self, mark: crate::cursor::CursorMark) -> TokenSpan {
        let id = crate::cursor::TokenCursor::mark_pos(mark).min(self.table.len() - 1) as TokenId;
        self.span_of(id)
    }

    /// Span of the token that would be at a mark's position after the cursor
    /// moved past it (used for `beforeParam` ends).
    fn peek_span_at_mark(&self, mark: crate::cursor::CursorMark) -> TokenSpan {
        self.span_at_mark(mark)
    }

    /// Official `parseInvalidTopLevelDeclaration`.
    pub(crate) fn parse_invalid_top_level_declaration(&mut self) {
        let at = self.peek_span();
        let kind = self.peek_kind();
        // A zero-width scanner-recovery closer (e.g. the EOF-pinned `)` of
        // a `(` a declaration recovery took over) is a recovery artifact:
        // the scanner channel already carries its unbalanced-group face, so
        // it is consumed silently here — the layer that actually sees it —
        // instead of stacking a declaration face on top. The span is
        // genuinely zero-width (the scanner pushes recovery closers with
        // length 0, lexer/mod.rs:910; real tokens are never zero-width).
        if matches!(
            kind,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly
        ) && at.range.length == 0
        {
            self.cursor.advance();
            return;
        }
        if kind == TokenKind::Semicolon {
            self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
        } else {
            self.report(msg::expected_executable(), at);
        }
        if kind == TokenKind::OpenCurly {
            self.skip_block(crate::message::BlockKind::Body);
            self.listener.handle_invalid_top_level_declaration(at);
        } else {
            self.listener.handle_invalid_top_level_declaration(at);
            self.cursor.advance();
        }
    }

    /// Diet expression consumption: group-aware skip of an expression-shaped
    /// token run (the reduced stand-in for the official skipped expression
    /// parse; stops at `,` `;` `)` `]` `}` and EOF at depth zero).
    pub(crate) fn skip_expression_diet(&mut self) {
        while !self.cursor.at_eof() {
            let kind = self.peek_kind();
            match kind {
                TokenKind::Comma
                | TokenKind::Semicolon
                | TokenKind::CloseParen
                | TokenKind::CloseSquare
                | TokenKind::CloseCurly => return,
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    if let Some(end) = self.current_matching_end() {
                        self.skip_to_token(end);
                        self.cursor.advance();
                        continue;
                    }
                    self.cursor.advance();
                }
                _ => {
                    self.cursor.advance();
                }
            }
        }
    }
}

/// `isUnaryMinus`: `unary` immediately followed by `-`.
fn is_unary_minus_ahead<L: ParserListener>(parser: &Parser<'_, '_, '_, L>) -> bool {
    // Official `isUnaryMinus` (parser_impl.dart:5208): the legacy
    // `operator unary-(` spelling — identifier `unary` followed by `-`.
    is_identifier(parser.peek_kind())
        && parser.cursor.lexeme(parser.cursor.peek().span.id) == "unary"
        && parser.cursor.look_kind(1) == TokenKind::Minus
}

/// `isModifier` (scanner `type.isModifier` keyword set).
pub(crate) fn is_modifier_kind(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            Keyword::Abstract
                | Keyword::Augment
                | Keyword::Const
                | Keyword::Covariant
                | Keyword::External
                | Keyword::Final
                | Keyword::Late
                | Keyword::Required
                | Keyword::Static
                | Keyword::Var
        )
    )
}

/// `isKeywordOrIdentifier` (any keyword or identifier).
pub(crate) fn is_keyword_or_identifier(kind: TokenKind) -> bool {
    is_identifier(kind) || matches!(kind, TokenKind::Keyword(_))
}

/// `isUserDefinableOperator` on a kind (shared with the record-type
/// follower check).
pub(crate) fn is_user_definable_operator_kind(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Tilde
            | TokenKind::TildeSlash
            | TokenKind::EqEqEq
            | TokenKind::BangEqEq
            | TokenKind::Lt
            | TokenKind::Gt
            | TokenKind::LtEq
            | TokenKind::GtEq
            | TokenKind::LtLt
            | TokenKind::GtGt
            | TokenKind::GtGtGt
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Bar
            | TokenKind::Caret
            | TokenKind::Ampersand
            | TokenKind::LtLtEq
            | TokenKind::GtGtEq
            | TokenKind::GtGtGtEq
            | TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::TildeSlashEq
            | TokenKind::BarEq
            | TokenKind::CaretEq
            | TokenKind::AmpersandEq
            | TokenKind::Index
            | TokenKind::IndexEq
    )
}
