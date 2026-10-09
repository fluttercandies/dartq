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

use dartq_scanner::{Keyword, TokenKind};

use crate::listener::{
    AssertKind, ConstructorReferenceContext, DeclarationKind, IdentifierContext, MemberKind,
    ParserListener,
};
use crate::message::msg;
use crate::parser::{AsyncModifier, Parser};
use crate::recovery::is_identifier;
use crate::token_id::{TokenId, TokenSpan};
use crate::type_args::TypeParamOrArg;
use crate::types::TypeInfo;

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
}

impl<'s, L: ParserListener> Parser<'s, '_, L> {
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
            && let Some(after_pattern) = self.skip_outer_pattern()
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
                    // Declaration missing its identifier: synthesize one.
                    let at = self.peek_span();
                    self.report(msg::missing_identifier(), at);
                    let id = self
                        .cursor
                        .insert_synthetic(TokenKind::Identifier, at.range.offset);
                    self.cursor.advance();
                    let _ = id;
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
                None,
                name_is_recovered,
            );
        }
    }

    /// Top-level modifier walk (official head of `parseTopLevelMemberImpl`).
    fn walk_top_level_member_modifiers(&mut self) -> MemberModifiers {
        let mut mods = MemberModifiers::default();
        let mut vfc_done = false;
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::External) if mods.external.is_none() => {
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
                TokenKind::Keyword(Keyword::Augment) if mods.augment.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.augment = Some(t);
                }
                TokenKind::Keyword(Keyword::Abstract) if mods.abstract_kw.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("abstract"), t);
                    mods.abstract_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Static) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("static"), t);
                    mods.static_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Covariant) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("covariant"), t);
                    mods.covariant = Some(t);
                }
                TokenKind::Keyword(Keyword::Late) if mods.late_kw.is_none() => {
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

        let name = match get_or_set {
            Some(get_set) => {
                // Official `token = getOrSet; ensureIdentifierPotentiallyRecovered`:
                // the identifier event is the name AFTER the `get`/`set`
                // introducer.
                self.cursor.advance();
                self.ensure_identifier_recovered(IdentifierContext::TopLevelFunctionDeclaration);
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
            self.parse_method_type_var(name);
        } else {
            self.listener.handle_no_type_variables(self.peek_span());
        }
        let formals_end =
            self.parse_getter_or_formal_parameters(name, is_getter, MemberKind::TopLevelMethod);
        let _ = formals_end;

        let saved_async = self.async_state;
        let async_end = self.parse_async_modifier_opt();
        if get_or_set.is_some_and(|t| self.cursor.lexeme(t.id) == "set")
            && self.async_state != AsyncModifier::Sync
        {
            self.report(msg::setter_not_sync(), async_end);
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

        let mods = self.walk_class_member_modifiers();

        self.listener.begin_member();

        let before_type = self.cursor.mark();

        // Pattern variables outside functions (`final (a, b) = ...`) recover
        // as fields with a synthetic name (official synthetic-name +
        // dropRange; the pattern tokens are diet-skipped).
        if let Some(vfc) = mods.var_final_or_const
            && self.cursor.lexeme(vfc.id) != "const"
            && let Some(after_pattern) = self.skip_outer_pattern()
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
                        if self.cursor.progress()
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
                    // Fall through: `factory` used as an identifier.
                }
                TokenKind::Keyword(Keyword::Operator) => {
                    let follower = self.cursor.look_kind(1);
                    if Self::is_user_definable_operator(follower)
                        && !matches!(self.cursor.look_kind(2), TokenKind::Lt)
                    {
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

    /// Class-member modifier walk (official head of
    /// `parseClassOrMixinOrExtensionOrEnumMemberImpl` +
    /// `parseClassMemberModifiers`).
    fn walk_class_member_modifiers(&mut self) -> MemberModifiers {
        let mut mods = MemberModifiers::default();
        let mut static_seen = false;
        let mut covariant_seen = false;
        let mut vfc_seen = false;
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::External) if mods.external.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.augment.is_some() {
                        self.report(msg::modifier_out_of_order("external", "augment"), t);
                    }
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("external", earlier), t);
                    }
                    mods.external = Some(t);
                }
                TokenKind::Keyword(Keyword::Augment) if mods.augment.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.augment = Some(t);
                }
                TokenKind::Keyword(Keyword::Abstract) if mods.abstract_kw.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.abstract_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Static)
                    if !static_seen && mods.abstract_kw.is_none() =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if covariant_seen {
                        self.report(msg::covariant_and_static(), t);
                    }
                    if mods.augment.is_some() {
                        self.report(msg::modifier_out_of_order("static", "augment"), t);
                    }
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("static", earlier), t);
                    }
                    mods.static_kw = Some(t);
                    static_seen = true;
                }
                TokenKind::Keyword(Keyword::Covariant) if !covariant_seen => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.static_kw.is_some() {
                        self.report(msg::covariant_and_static(), t);
                    }
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("covariant", earlier), t);
                    }
                    mods.covariant = Some(t);
                    covariant_seen = true;
                }
                TokenKind::Keyword(Keyword::Late) if mods.late_kw.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if let Some(vfc) = mods.var_final_or_const {
                        let earlier = self.cursor.lexeme(vfc.id);
                        self.report(msg::modifier_out_of_order("late", earlier), t);
                    }
                    mods.late_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Final) if !vfc_seen => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.final_kw = Some(t);
                    if mods.late_kw.is_some() {
                        self.report(msg::modifier_out_of_order("final", "late"), t);
                    }
                    if let Some(v) = mods.var_kw {
                        self.report(msg::final_and_var(), v);
                    }
                    mods.var_final_or_const = Some(t);
                    vfc_seen = true;
                }
                TokenKind::Keyword(Keyword::Var) if !vfc_seen => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.var_kw = Some(t);
                    mods.var_final_or_const = Some(t);
                    vfc_seen = true;
                }
                TokenKind::Keyword(Keyword::Const) if !vfc_seen && !covariant_seen => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if let Some(l) = mods.late_kw {
                        self.report(msg::conflicting_modifiers("const", "late"), l);
                    }
                    if let Some(f) = mods.final_kw {
                        self.report(msg::const_and_final(), f);
                    }
                    if let Some(v) = mods.var_kw {
                        self.report(msg::conflicting_modifiers("const", "var"), v);
                    }
                    mods.var_final_or_const = Some(t);
                    vfc_seen = true;
                }
                TokenKind::Keyword(Keyword::Required) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("required"), t);
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
            if Self::is_user_definable_operator(follower)
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
            // the name throughout).
            self.cursor.jump_to(name.id);
            self.cursor.advance();
        }

        let mut has_qualified_name = false;
        if is_operator {
            // Name already consumed above.
        } else if new_kw.is_some() {
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
            self.parse_method_type_var(name);
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
        let begin_param = self.parse_getter_or_formal_parameters(
            name_for_formals,
            is_considered_getter,
            member_kind,
        );

        let begin_initializers = self.parse_initializers_opt();

        let saved_async = self.async_state;
        let async_end = self.parse_async_modifier_opt();
        if get_or_set.is_some_and(|t| self.cursor.lexeme(t.id) == "set")
            && self.async_state != AsyncModifier::Sync
        {
            self.report(msg::setter_not_sync(), async_end);
        }
        if mods.external.is_some() && self.peek_kind() != TokenKind::Semicolon {
            self.report(msg::external_method_with_body(), self.peek_span());
        }
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
        self.parse_method_type_var(name_end);
        self.parse_formal_parameters_required_opt(MemberKind::Factory);
        let async_end = self.parse_async_modifier_opt();
        if self.async_state != AsyncModifier::Sync {
            self.report(msg::factory_not_sync(), async_end);
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
        // Skip straight to the operator token and parse it as a method.
        while !Self::is_user_definable_operator(self.peek_kind())
            && !matches!(
                self.peek_kind(),
                TokenKind::EqEqEq | TokenKind::BangEqEq | TokenKind::Eof
            )
        {
            self.force_advance();
        }
        let at = self.peek_span();
        self.report(msg::missing_operator_keyword(), at);
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
                return;
            }
            _ => {}
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
        } else if self.cursor.progress() == crate::cursor::TokenCursor::progress_at(before_type)
            && !matches!(next_kind, TokenKind::CloseCurly)
        {
            self.report(
                msg::expected_class_member(self.cursor.lexeme(next_span.id)),
                next_span,
            );
            self.listener.handle_invalid_member(next_span);
            self.cursor.advance();
        } else if !matches!(next_kind, TokenKind::CloseCurly | TokenKind::Eof) {
            self.parse_fields(kind, before_type, mods, before_type, type_info, None, false);
        } else {
            self.report(
                msg::expected_class_member(self.cursor.lexeme(next_span.id)),
                next_span,
            );
            self.listener.handle_invalid_member(next_span);
        }
    }

    /// `reportAndSkipClassInClass`/`reportAndSkipEnumInClass` shape: skip
    /// `name { body }` when it looks like a nested declaration.
    fn skip_member_shaped_declaration(&mut self) {
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
                self.report(msg::missing_const_final_var_or_type(), self.peek_span());
            }
        } else if let Some(vfc) = mods.var_final_or_const
            && self.cursor.lexeme(vfc.id) == "var"
        {
            self.report(msg::type_after_var(), vfc);
        }
        if let (Some(a), true) = (mods.abstract_kw, mods.external.is_some()) {
            self.report(msg::abstract_external_field(), a);
        }

        self.cursor.reset(before_type);
        type_info.parse_type(self);

        let context = match kind {
            DeclarationKind::TopLevel => IdentifierContext::TopLevelVariableDeclaration,
            _ => IdentifierContext::FieldDeclaration,
        };
        let first_name = match synthetic_name {
            Some(name) => name,
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

        let mut field_count = 1usize;
        self.parse_field_initializer_opt(first_name, kind, &mods);
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            let name = self.ensure_identifier(context);
            self.parse_field_initializer_opt(name, kind, &mods);
            field_count += 1;
        }
        let end = self.ensure_semicolon();

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
    fn parse_field_initializer_opt(
        &mut self,
        name: TokenSpan,
        kind: DeclarationKind,
        mods: &MemberModifiers,
    ) {
        if self.peek_kind() == TokenKind::Eq {
            let assignment = self.peek_span();
            self.listener.begin_field_initializer(assignment);
            self.cursor.advance();
            let end = self.parse_expression();
            self.listener.end_field_initializer(assignment, end);
        } else {
            if let Some(vfc) = mods.var_final_or_const {
                let lexeme = self.cursor.lexeme(vfc.id);
                let name_text = self.cursor.lexeme(name.id);
                if lexeme == "const" {
                    self.report(msg::const_field_without_initializer(name_text), name);
                } else if lexeme == "final"
                    && kind == DeclarationKind::TopLevel
                    && mods.late_kw.is_none()
                    && mods.abstract_kw.is_none()
                    && mods.external.is_none()
                {
                    self.report(msg::final_field_without_initializer(name_text), name);
                }
            }
            let next = self.peek_span();
            self.listener.handle_no_field_initializer(next);
        }
    }

    /// Official `parseMethodTypeVar` (`<...>` after a method name).
    fn parse_method_type_var(&mut self, name: TokenSpan) {
        if self.peek_kind() == TokenKind::Bang {
            let at = self.peek_span();
            self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
            self.cursor.advance();
        }
        if self.peek_kind() != TokenKind::Lt {
            return;
        }
        let type_param = self.compute_type_param_or_arg_on_lt(true);
        if !matches!(type_param, TypeParamOrArg::None) {
            type_param.parse_variables(self, false);
        }
        if self.peek_kind() == TokenKind::Eq {
            let at = self.peek_span();
            self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
            self.cursor.advance();
        }
        let _ = name;
    }

    /// Official `parseQualifiedRestOpt`: `. identifier` suffix.
    fn parse_qualified_rest_opt(&mut self, context: IdentifierContext) {
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
            self.report(msg::missing_parameters(kind), name);
            let at = self.peek_span();
            let open = self.synthetic(TokenKind::OpenParen, at.range.offset);
            let close = self.synthetic(TokenKind::CloseParen, at.range.offset);
            self.listener.begin_formal_parameters(open, kind);
            self.listener.end_formal_parameters(0, open, close, kind);
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
        loop {
            if self.is_bailed() {
                // Terminal bailout: unwind without consumption (see
                // `parse_list_literal` in literals.rs).
                break;
            }
            last_end = self.parse_initializer(last_end);
            count += 1;
            if self.peek_kind() != TokenKind::Comma {
                // Recovery (:4770-4798): the head looks like another entry
                // (`assert(`, `this`/`super` + `(`/`.`, `name =`) — a comma
                // went missing; report and insert one.
                if self.initializer_head_continues() {
                    self.report(msg::expected_after_but_got(","), last_end);
                    let at = self.peek_span().range.offset;
                    self.synthetic(TokenKind::Comma, at);
                    self.cursor.advance();
                    continue;
                }
                break;
            }
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
                // reparses the `this.name` head natively (`.new` included).
                let name = self.cursor.look_kind(2);
                if !(is_identifier(name) || name == TokenKind::Keyword(Keyword::New)) {
                    // Official `insertSyntheticIdentifier(dot, fieldInitializer)`
                    // (:4841): the context recovery template.
                    let at = self.peek_span_at(2);
                    self.report(msg::missing_identifier(), at);
                    self.synthetic(TokenKind::Identifier, at.range.offset);
                }
                let follower = self.cursor.look_kind(3);
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
            let at = self.peek_span_at(1);
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
            // is the assignment target — insert `= <synthetic name>` after
            // it. `Token.isOperator` covers real operators; group punctuation
            // and `.` are excluded (`kind_precedence` is `Some` for `.`,
            // matching the explicit `!next2.isA(PERIOD)` rider).
            let follower = self.cursor.look_kind(1);
            if crate::expressions::kind_precedence(follower).is_none() {
                self.report(msg::missing_assignment_in_initializer(), head);
                let at = self.peek_span_at(1);
                self.synthetic(TokenKind::Eq, at.range.offset);
                self.synthetic(TokenKind::Identifier, at.range.offset);
                return self.parse_initializer_expression_rest();
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
        let end = self.parse_initializer_expression_rest();
        let after = self.peek_kind();
        if after == TokenKind::OpenCurly || after == TokenKind::Function {
            self.report(msg::redirecting_constructor_with_body(), self.peek_span());
        }
        end
    }

    /// Official `parseSuperInitializerExpression` (:4918): lookahead over
    /// `super (. name)?` to validate the shape, then the expression re-read.
    fn parse_super_initializer_expression(&mut self) -> TokenSpan {
        let mut n = 1;
        if self.cursor.look_kind(n) == TokenKind::Period {
            n += 1;
            let name = self.cursor.look_kind(n);
            if !(is_identifier(name) || name == TokenKind::Keyword(Keyword::New)) {
                // Official `expressionContinuation.ensureIdentifier`.
                let at = self.peek_span_at(n);
                self.report(msg::missing_identifier(), at);
                self.synthetic(TokenKind::Identifier, at.range.offset);
            }
            n += 1;
        }
        let next = self.cursor.look_kind(n);
        if next != TokenKind::OpenParen {
            if next == TokenKind::QuestionPeriod {
                // `super?.` — the error itself is reported by the
                // `parseSuperExpression` re-read; only a missing member name
                // gets the synthetic here (:4936-4944).
                let name = self.cursor.look_kind(n + 1);
                if !(is_identifier(name) || name == TokenKind::Keyword(Keyword::New)) {
                    let at = self.peek_span_at(n + 1);
                    self.synthetic(TokenKind::Identifier, at.range.offset);
                }
            } else if next == TokenKind::Eq {
                let before_eq = self.cursor.look_kind(n - 1);
                if before_eq != TokenKind::Keyword(Keyword::Super) {
                    // `super.name = ...` — parseExpression reports the
                    // assignment itself; this is the declaring-class rider.
                    self.report(
                        msg::field_initialized_outside_declaring_class(),
                        self.peek_span_at(n - 1),
                    );
                }
            } else {
                // `expectedAfterButGot('(')` + `insertParens(includeIdentifier:
                // false)` (:4948-4952): wrap the follower in empty parens.
                let at = self.peek_span_at(n);
                self.report(msg::expected_after_but_got("("), at);
                self.synthetic(TokenKind::OpenParen, at.range.offset);
                self.synthetic(TokenKind::CloseParen, at.range.offset);
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

    /// Official `parseAsyncModifierOpt`; updates and returns with the new
    /// [`AsyncModifier`] state.
    pub(crate) fn parse_async_modifier_opt(&mut self) -> TokenSpan {
        let mut async_kw: Option<TokenSpan> = None;
        let mut star: Option<TokenSpan> = None;
        self.async_state = AsyncModifier::Sync;
        let mut end = self.peek_span();
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
                end = star.unwrap_or(async_kw.unwrap());
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
                end = star.unwrap_or(async_kw.unwrap());
            }
            _ => {}
        }
        self.listener.handle_async_modifier(async_kw, star);
        if self.async_state != AsyncModifier::Sync && self.peek_kind() == TokenKind::Semicolon {
            self.report(msg::abstract_not_sync(), self.peek_span());
        }
        end
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

    fn peek_is_reserved_at(&self, n: usize) -> bool {
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
        let after = self.cursor.look_kind(0);
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
            after = self.cursor.look_kind(1);
        }
        if after == TokenKind::Lt {
            // Skip a `<...>` group when it is scanner-paired.
            if let Some(end) = self.current_group_end_at(0) {
                let after_group = self.kind_after(end);
                if after_group == TokenKind::Eq {
                    after = TokenKind::Eq;
                } else {
                    after = after_group;
                }
            }
        }
        if after == TokenKind::OpenParen {
            if let Some(close) = self.current_group_end_at(0)
                && self.kind_after(close) == TokenKind::Colon
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

    /// Matching close id for a group at cursor-relative offset `n`.
    fn current_group_end_at(&self, n: usize) -> Option<TokenId> {
        // Only valid right after a clean consume; n > 0 falls back to None
        // (queue-aware group matching is unnecessary for the constructor
        // check's shapes).
        let _ = n;
        self.current_matching_end()
    }

    /// Kind right after absolute id `end`.
    fn kind_after(&self, end: TokenId) -> TokenKind {
        let offset = self.offset_of(end);
        if self.cursor.peek_id() > end {
            return self.cursor.look_kind(0);
        }
        self.kind_at(offset + 1)
    }

    /// `skipOuterPattern` (compute-only): returns the id of the pattern's
    /// last token when the cursor currently starts an outer pattern.
    fn skip_outer_pattern(&self) -> Option<TokenId> {
        match self.peek_kind() {
            TokenKind::OpenSquare | TokenKind::OpenCurly | TokenKind::OpenParen => {
                self.current_matching_end()
            }
            _ => None,
        }
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
        if kind == TokenKind::Semicolon {
            self.report(msg::unexpected_token(self.cursor.lexeme(at.id)), at);
        } else {
            self.report(msg::expected_declaration(self.cursor.lexeme(at.id)), at);
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
        let before = self.cursor.progress();
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
            if self.no_progress(before) {
                // Cannot happen (advance always moves), but keep the guard
                // symmetric with the other loops.
                self.force_advance();
            }
        }
    }
}

/// `isUnaryMinus`: `unary` immediately followed by `-`.
fn is_unary_minus_ahead<L: ParserListener>(parser: &Parser<'_, '_, L>) -> bool {
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
