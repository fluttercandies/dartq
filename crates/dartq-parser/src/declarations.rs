//! Top-level declaration dispatch and the declaration families (official
//! `parseTopLevelDeclarationImpl` `parser_impl.dart:569`,
//! `parseTopLevelKeywordDeclaration` :687, `parseTypedef` :1470,
//! `parseEnum` :2616, `parseClassOrNamedMixinApplication` :2957,
//! `parseClass` :3099, `parseMixin` :3391, `parseExtension` :3613,
//! `parseExtensionTypeDeclaration` :3960, and the header-clause helpers).
//!
//! Directives (`import`/`export`/`part`/`library`) dispatch to the
//! dedicated directive grammar (`directives.rs`, #300); this module places
//! the `DirectiveContext` ordering checks at the official call sites.

use dartq_scanner::{Keyword, TokenKind};

use crate::listener::{
    ConstructorReferenceContext, DeclarationHeaderKind, DeclarationKind, IdentifierContext,
    ParserListener,
};
use crate::members::is_keyword_or_identifier;
use crate::members::is_modifier_kind;
use crate::message::msg;
use crate::parser::Parser;
use crate::recovery::is_identifier;
use crate::token_id::TokenSpan;
use crate::type_args::TypeParamOrArg;
use crate::types::TypeInfo;

/// `isTopLevelKeyword` (scanner keyword set).
fn is_top_level_keyword(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(
            Keyword::Class
                | Keyword::Enum
                | Keyword::Export
                | Keyword::Extension
                | Keyword::Import
                | Keyword::Library
                | Keyword::Mixin
                | Keyword::Part
                | Keyword::Typedef
        )
    )
}

/// Header clauses already seen during class-header recovery.
#[derive(Default)]
struct HeaderClauses {
    extends: bool,
    with: bool,
    implements: bool,
}

impl<'s, L: ParserListener> Parser<'s, '_, L> {
    /// One top-level declaration (official `parseTopLevelDeclarationImpl`).
    pub(crate) fn parse_top_level_declaration(&mut self) {
        self.parse_metadata_star();

        let mark = self.cursor.mark();
        let begin = self.peek_span();

        if is_top_level_keyword(self.peek_kind()) {
            self.parse_top_level_keyword_declaration(mark, begin, None, None, None);
            return;
        }

        // var/late/final/const head straight to member parsing (official
        // special case: `final class` / `const class` fall through to the
        // member walk and report below).
        let kind = self.peek_kind();
        if matches!(kind, TokenKind::Keyword(Keyword::Var | Keyword::Late)) {
            self.check_declaration();
            self.parse_top_level_member_impl();
            return;
        }
        if kind == TokenKind::Keyword(Keyword::Final)
            && !matches!(
                self.cursor.look_kind(1),
                TokenKind::Keyword(Keyword::Class | Keyword::Mixin | Keyword::Enum)
            )
        {
            self.check_declaration();
            self.parse_top_level_member_impl();
            return;
        }
        if kind == TokenKind::Keyword(Keyword::Const)
            && self.cursor.look_kind(1) != TokenKind::Keyword(Keyword::Class)
        {
            self.check_declaration();
            self.parse_top_level_member_impl();
            return;
        }

        // Walk real modifiers (lookahead only; nothing dispatched yet).
        while is_modifier_kind(self.peek_kind()) {
            self.cursor.advance();
        }

        // sealed / base / interface pseudo-keyword modifiers.
        let mut sealed: Option<TokenSpan> = None;
        let mut base: Option<TokenSpan> = None;
        let mut interface: Option<TokenSpan> = None;
        if self.peek_is_pseudo(Keyword::Sealed) {
            sealed = Some(self.peek_span());
            self.cursor.advance();
            if !matches!(
                self.peek_kind(),
                TokenKind::Keyword(Keyword::Class | Keyword::Mixin | Keyword::Enum)
            ) && self.peek_is_pseudo(Keyword::Abstract)
                && self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::Class)
            {
                self.cursor.advance(); // `abstract` (deferred error)
            }
        } else if self.peek_is_pseudo(Keyword::Base) {
            base = Some(self.peek_span());
            self.cursor.advance();
        } else if self.peek_is_pseudo(Keyword::Interface) {
            interface = Some(self.peek_span());
            self.cursor.advance();
        }

        if is_top_level_keyword(self.peek_kind()) {
            self.parse_top_level_keyword_declaration(mark, begin, sealed, base, interface);
            return;
        }

        // Not a keyword declaration: rewind and parse as a member (the
        // member impl re-walks the modifiers itself).
        self.cursor.reset(mark);
        let operator_ahead = self.cursor.look_kind(0);
        if Self::is_operator_token_at_top(operator_ahead)
            && self.cursor.look_kind(1) == TokenKind::OpenParen
        {
            // Top-level operator: report and recover as invalid.
            let at = self.peek_span();
            self.report(msg::top_level_operator(), at);
            self.listener.handle_invalid_top_level_declaration(at);
            self.force_advance();
            return;
        }
        self.check_declaration();
        self.parse_top_level_member_impl();
    }

    fn peek_is_pseudo(&self, kw: Keyword) -> bool {
        self.peek_kind() == TokenKind::Keyword(kw) && is_identifier(self.peek_kind())
    }

    fn is_operator_token_at_top(kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::Tilde
                | TokenKind::TildeSlash
                | TokenKind::EqEq
                | TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::LtEq
                | TokenKind::GtEq
                | TokenKind::LtLt
                | TokenKind::GtGt
                | TokenKind::GtGtGt
                | TokenKind::Bar
                | TokenKind::Caret
                | TokenKind::Ampersand
                | TokenKind::Index
                | TokenKind::EqEqEq
                | TokenKind::BangEqEq
        )
    }

    /// Official `parseTopLevelKeywordDeclaration`; the cursor sits on the
    /// keyword.
    fn parse_top_level_keyword_declaration(
        &mut self,
        modifier_start: crate::cursor::CursorMark,
        begin: TokenSpan,
        sealed: Option<TokenSpan>,
        base: Option<TokenSpan>,
        interface: Option<TokenSpan>,
    ) {
        let keyword = self.peek_span();
        match self.peek_kind() {
            TokenKind::Keyword(Keyword::Class) => {
                self.check_declaration();
                let mods = self.parse_class_modifiers(modifier_start, keyword);
                self.parse_class_or_named_mixin_application(
                    begin,
                    mods.abstract_kw,
                    sealed,
                    base,
                    interface,
                    mods.final_kw,
                    mods.augment,
                    None,
                    keyword,
                );
            }
            TokenKind::Keyword(Keyword::Enum) => {
                self.check_declaration();
                let mods = self.parse_declaration_modifiers(modifier_start, keyword);
                // Official family-row order: base, final, interface, sealed
                // ("Enums can't declare any explicit modifier",
                // parser_impl.dart:689-701). `base`/`interface`/`sealed` are
                // the dispatch scan's pseudo-keyword captures; `final` is the
                // declaration walk's capture.
                if let Some(b) = base {
                    self.report(msg::base_enum(), b);
                }
                if let Some(f) = mods.final_kw {
                    self.report(msg::final_enum(), f);
                }
                if let Some(i) = interface {
                    self.report(msg::interface_enum(), i);
                }
                if let Some(s) = sealed {
                    self.report(msg::sealed_enum(), s);
                }
                self.pending_augment = mods.augment;
                self.parse_enum(begin, keyword);
            }
            _ => {
                // The remaining top-level keywords are built-ins usable as
                // identifiers ("abstract<T>() => 0;").
                let next = self.cursor.look_kind(1);
                let typedef_with_record = self.peek_kind() == TokenKind::Keyword(Keyword::Typedef)
                    && next == TokenKind::OpenParen
                    && self.typedef_looks_like_record();
                if (next == TokenKind::OpenParen || next == TokenKind::Period)
                    && !typedef_with_record
                {
                    self.cursor.reset(modifier_start);
                    self.parse_top_level_member_impl();
                    return;
                }
                if next == TokenKind::Lt {
                    if self.peek_kind() == TokenKind::Keyword(Keyword::Extension) {
                        // `extension<T> on ...` — the name is optional
                        // (official :771-782: `endGroup.next.isA(Keyword.ON)`).
                        if let Some(end_group) = self.angle_group_after(1)
                            && self.kind_at_abs(end_group + 1) == TokenKind::Keyword(Keyword::On)
                        {
                            self.check_declaration();
                            let mods = self.parse_declaration_modifiers(modifier_start, keyword);
                            self.parse_extension(begin, mods.augment, keyword);
                            return;
                        }
                    } else if self.peek_kind() == TokenKind::Keyword(Keyword::Typedef) {
                        // `typedef <...> =` — missing the typedef name
                        // (official :763-773: endGroup followed by `=`).
                        if let Some(end_group) = self.angle_group_after(1)
                            && self.kind_at_abs(end_group + 1) == TokenKind::Eq
                        {
                            self.check_declaration();
                            let mods = self.parse_declaration_modifiers(modifier_start, keyword);
                            if let Some(f) = mods.final_kw {
                                self.report(msg::extraneous_modifier("final"), f);
                            }
                            self.parse_typedef(keyword);
                            return;
                        }
                    }
                    self.check_declaration();
                    self.cursor.reset(modifier_start);
                    self.parse_top_level_member_impl();
                    return;
                }
                match self.peek_kind() {
                    TokenKind::Keyword(Keyword::Typedef) => {
                        let mods = self.parse_declaration_modifiers(modifier_start, keyword);
                        if let Some(f) = mods.final_kw {
                            self.report(msg::extraneous_modifier("final"), f);
                        }
                        self.check_declaration();
                        self.parse_typedef(keyword);
                    }
                    TokenKind::Keyword(Keyword::Mixin) => {
                        if self.cursor.look_kind(1) == TokenKind::Keyword(Keyword::Class) {
                            self.check_declaration();
                            let class_keyword = self.look_span_at(1);
                            let mods = self.parse_class_modifiers(modifier_start, class_keyword);
                            // Official `_handleModifiersForClassDeclaration`
                            // (parser_impl.dart:850-876) starts
                            // `parseClassOrNamedMixinApplication` AT the
                            // `class` token (`Token token = classKeyword`,
                            // :2969); the modifier walk stops on the `mixin`
                            // head (`_parseModifiers`, modifier_context.dart
                            // :345-384 — `mixin` is not in the modifier set),
                            // leaving the cursor on it. Consume `mixin` so the
                            // cursor sits on `class` — the same position the
                            // plain-`class` dispatch leaves. Without this the
                            // name step took `class` as the class name
                            // (`final mixin class M {}` reported
                            // `expected_identifier_but_got_keyword` at `class`
                            // and `unexpected_token` at `M`).
                            self.cursor.advance(); // `mixin`
                            if let Some(f) = mods.final_kw {
                                self.report(msg::final_mixin_class(), f);
                            }
                            if let Some(i) = interface {
                                self.report(msg::interface_mixin_class(), i);
                            }
                            if let Some(s) = sealed {
                                self.report(msg::sealed_mixin_class(), s);
                            }
                            self.parse_class_or_named_mixin_application(
                                begin,
                                mods.abstract_kw,
                                sealed,
                                base,
                                interface,
                                mods.final_kw,
                                mods.augment,
                                Some(keyword),
                                class_keyword,
                            );
                        } else {
                            let mods = self.parse_declaration_modifiers(modifier_start, keyword);
                            if let Some(f) = mods.final_kw {
                                self.report(msg::final_mixin(), f);
                            }
                            if let Some(i) = interface {
                                self.report(msg::interface_mixin(), i);
                            }
                            if let Some(s) = sealed {
                                self.report(msg::sealed_mixin(), s);
                            }
                            self.check_declaration();
                            self.parse_mixin(begin, mods.augment, base, keyword);
                        }
                    }
                    TokenKind::Keyword(Keyword::Extension) => {
                        let mods = self.parse_declaration_modifiers(modifier_start, keyword);
                        self.check_declaration();
                        self.parse_extension(begin, mods.augment, keyword);
                    }
                    TokenKind::Keyword(Keyword::Import) => {
                        let _ = self.parse_directive_modifiers(modifier_start, keyword, false);
                        self.check_import(keyword);
                        self.parse_import();
                    }
                    TokenKind::Keyword(Keyword::Export) => {
                        let _ = self.parse_directive_modifiers(modifier_start, keyword, false);
                        self.check_export(keyword);
                        self.parse_export();
                    }
                    TokenKind::Keyword(Keyword::Part) => {
                        let _ = self.parse_directive_modifiers(modifier_start, keyword, false);
                        self.parse_part_or_part_of();
                    }
                    TokenKind::Keyword(Keyword::Library) => {
                        // Official order (:837-845): the ordering check runs
                        // BEFORE the modifier walk for `library` only.
                        self.check_library(keyword);
                        let augment = self.parse_directive_modifiers(modifier_start, keyword, true);
                        match augment {
                            Some(aug) => self.parse_library_augmentation(aug, keyword),
                            None => self.parse_library_name(keyword),
                        }
                    }
                    _ => {
                        self.check_declaration();
                        self.cursor.reset(modifier_start);
                        self.parse_top_level_member_impl();
                    }
                }
            }
        }
    }

    /// `typedef (` followed by `id`/`?id` and a record-shaped type.
    fn typedef_looks_like_record(&mut self) -> bool {
        // Cursor on `typedef`; the `(` is at offset 1.
        let Some(end_paren) = self.table_matching_end_at(1) else {
            return false;
        };
        let after = self.kind_at_abs(end_paren + 1);
        let is_id_or_question_id = is_identifier(after)
            || (after == TokenKind::Question && is_identifier(self.kind_at_abs(end_paren + 2)));
        if !is_id_or_question_id {
            return false;
        }
        let type_info = self.compute_type(false, false);
        matches!(&type_info, TypeInfo::Complex(c) if c.is_record_type)
    }

    // `min` bounds the offset by the table length; token tables are
    // scanner-bounded far below `u32::MAX`.
    #[allow(clippy::cast_possible_truncation)]
    fn table_matching_end_at(&self, n: usize) -> Option<usize> {
        let id = (self.cursor.real_pos() + n).min(self.table.len() - 1);
        self.cursor
            .table_matching_end(id as crate::token_id::TokenId)
            .map(|e| self.offset_of(e))
    }

    fn kind_at_abs(&self, offset: usize) -> TokenKind {
        self.kind_at(offset)
    }

    /// Matching `>` for an angle group starting at cursor-relative offset
    /// `n` (queue must be empty).
    // `min` bounds the offset by the table length; token tables are
    // scanner-bounded far below `u32::MAX`.
    #[allow(clippy::cast_possible_truncation)]
    fn angle_group_after(&self, n: usize) -> Option<usize> {
        let lt = (self.cursor.real_pos() + n).min(self.table.len() - 1);
        self.cursor
            .table_matching_end(lt as crate::token_id::TokenId)
            .map(|e| self.offset_of(e))
    }

    /// `parseClassModifiers` + `_handleModifiersForClassDeclaration`
    /// bookkeeping. The cursor is rewound to `modifier_start`, the walk
    /// consumes the modifiers, and the captured tokens are returned.
    fn parse_class_modifiers(
        &mut self,
        modifier_start: crate::cursor::CursorMark,
        keyword: TokenSpan,
    ) -> ClassModifiers {
        self.cursor.reset(modifier_start);
        let mut mods = ClassModifiers::default();
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Abstract) if mods.abstract_kw.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.final_kw.is_some() || mods.covariant.is_some() {
                        let earlier = mods
                            .final_kw
                            .map(|t| self.cursor.lexeme(t.id))
                            .or_else(|| mods.covariant.map(|t| self.cursor.lexeme(t.id)))
                            .unwrap_or("var");
                        self.report(msg::modifier_out_of_order("abstract", earlier), t);
                    }
                    mods.abstract_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Augment) if mods.augment.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.augment = Some(t);
                }
                TokenKind::Keyword(Keyword::Final) if mods.final_kw.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.late_kw.is_some() {
                        self.report(msg::modifier_out_of_order("final", "late"), t);
                    }
                    mods.final_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Late) if mods.late_kw.is_none() => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.late_kw = Some(t);
                }
                TokenKind::Keyword(Keyword::Const) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::const_class(), t);
                }
                TokenKind::Keyword(Keyword::External) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::external_class(), t);
                }
                TokenKind::Keyword(Keyword::Covariant) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("covariant"), t);
                }
                TokenKind::Keyword(Keyword::Required) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("required"), t);
                }
                TokenKind::Keyword(Keyword::Static) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("static"), t);
                }
                TokenKind::Keyword(Keyword::Var) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("var"), t);
                }
                // Pseudo-keywords already captured by the dispatch walk
                // (`sealed`/`base`/`interface`): consume them here so the
                // walk ends on the declaration keyword.
                TokenKind::Keyword(Keyword::Sealed | Keyword::Base | Keyword::Interface) => {
                    self.cursor.advance();
                }
                _ => break,
            }
        }
        let _ = keyword;
        mods
    }

    /// Shared modifier walk for enum/mixin/extension/typedef (`reportTopLevel
    /// ModifierError` shape: const/external are reported against the
    /// declaration keyword). Also the base walk of the directive modifier
    /// families (`directives::parse_directive_modifiers`).
    pub(crate) fn parse_declaration_modifiers(
        &mut self,
        modifier_start: crate::cursor::CursorMark,
        keyword: TokenSpan,
    ) -> DeclModifiers {
        self.cursor.reset(modifier_start);
        let mut mods = DeclModifiers::default();
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Const) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("const"), t);
                }
                TokenKind::Keyword(Keyword::External) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    match self.cursor.lexeme(keyword.id) {
                        "enum" => self.report(msg::external_enum(), t),
                        "typedef" => self.report(msg::external_typedef(), t),
                        _ => self.report(msg::extraneous_modifier("external"), t),
                    }
                }
                TokenKind::Keyword(Keyword::Abstract) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("abstract"), t);
                }
                TokenKind::Keyword(Keyword::Covariant) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("covariant"), t);
                }
                TokenKind::Keyword(Keyword::Late) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("late"), t);
                }
                TokenKind::Keyword(Keyword::Required) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("required"), t);
                }
                TokenKind::Keyword(Keyword::Static) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("static"), t);
                }
                TokenKind::Keyword(Keyword::Var) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("var"), t);
                }
                TokenKind::Keyword(Keyword::Final) => {
                    // Captured: the declaration family decides whether `final`
                    // is an error (`final enum`, `final mixin`) or silently
                    // accepted (extension, official parseExtensionModifiers).
                    mods.final_kw = Some(self.peek_span());
                    self.cursor.advance();
                }
                TokenKind::Keyword(Keyword::Augment) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    mods.augment = Some(t);
                }
                // Pseudo-keywords already captured (and reported where
                // invalid) by the dispatch walk: consume and move on.
                TokenKind::Keyword(Keyword::Sealed | Keyword::Base | Keyword::Interface) => {
                    self.cursor.advance();
                }
                _ => break,
            }
        }
        mods
    }

    // ------------------------------------------------------------------
    // typedef
    // ------------------------------------------------------------------

    /// Official `parseTypedef`; the cursor sits on `typedef`.
    fn parse_typedef(&mut self, typedef_keyword: TokenSpan) {
        let augment = self.pending_augment.take();
        if let Some(a) = augment {
            self.report(msg::typedef_augmentation(), a);
        }
        self.listener
            .begin_uncategorized_top_level_declaration(typedef_keyword);
        self.listener.begin_typedef(typedef_keyword);
        // Official `parseTypedef` anchors on `typedefKeyword` itself
        // (parser_impl.dart:1470): some recovery routes reach us with the
        // cursor already past the keyword, so re-anchor before consuming.
        self.cursor.rewind_to(typedef_keyword.id);
        self.cursor.advance(); // `typedef`

        let type_info = self.compute_type(false, true);
        let mut equals: Option<TokenSpan> = None;

        // New-style vs old-style decision (official :1487-1518). The official
        // probes the group from `skipType(..).next!` — the token after the
        // (possibly empty) `typedef <type>` head — so the offset from the
        // cursor (still on the head's first token) depends on the head shape:
        // `NoType` skips to before the start (+1), a consumed type to its last
        // token (+2). The garbage-recovery recompute below re-anchors at the
        // cursor for the missing-name shape (`typedef <T> = ...`).
        let mut new_style = false;
        let head_end = type_info.skip_type(self);
        let head_offset = self.offset_of(head_end) + 2 - self.cursor.real_pos();
        let mut type_param = self.compute_type_param_or_arg_at(head_offset, true);
        if matches!(type_info, TypeInfo::NoType) {
            let skip = type_param.skip(self.peek_id());
            // Official `skip.next!.isA(TokenType.EQ)` (parser_impl.dart:1497):
            // the successor of the group's `skip` target in the official split
            // stream — the split remainder when the closer is a raw compound
            // (`=`/`>` for the fast shape, splitGtEq/splitGtGt,
            // type_info_impl.dart:1263/1293; the splitCloser remainder for a
            // full group, :1409), the real next token when the compound's last
            // closer closes the group. `type_arg_follower` is exactly that
            // read over our whole-compound table, so `typedef T<b>= int;`
            // decides new-style off the remainder `=` while `typedef T<b>>`
            // sees the remainder `>` and stays old-style.
            let next_kind =
                self.type_arg_follower(&type_param, self.cursor.real_pos() + head_offset);
            if next_kind == TokenKind::Eq || self.kind_at(self.offset_of(skip)) == TokenKind::Eq {
                new_style = true;
            } else if self.kind_at(self.offset_of(skip)) == TokenKind::Lt
                && let Some(end) = self
                    .cursor
                    .table_matching_end(self.real_id(self.offset_of(skip)))
                && self.kind_at(self.offset_of(end) + 1) == TokenKind::Eq
            {
                // Official recheck (:1510-1518): the group is recomputed from
                // the typedef keyword and the decision re-tests the recomputed
                // group's own successor — new-style only when that successor
                // is `=` (official "This if shouldn't be necessary, but let's
                // do it anyway").
                let new_type_param = self.compute_type_param_or_arg_at(0, true);
                if self.type_arg_follower(&new_type_param, self.cursor.real_pos()) == TokenKind::Eq
                {
                    type_param = new_type_param;
                    new_style = true;
                }
            }
        }

        let end;
        if new_style {
            let name = if self.peek_is_identifier() {
                self.ensure_identifier(IdentifierContext::TypedefDeclaration)
            } else {
                self.ensure_identifier_recovered(IdentifierContext::TypedefDeclaration)
            };
            let _ = name;
            // The missing-name recovery may have consumed the variable
            // group's own `<` as the name (the official consume-in-doubt
            // branch, identifier_context_impl.dart:39-42); official
            // `parseVariables` then re-uses that token as the group start
            // (`assert(identical(token.next!, start))` over the consumed
            // name, type_info_impl.dart:1477). Re-anchor the same way so the
            // group parses from its opener.
            if let crate::type_args::TypeParamOrArg::Complex(g) = &type_param
                && name.id == g.start
            {
                self.cursor.rewind_to(g.start);
            }
            type_param.parse_variables(self, false);
            // Official :1528-1534 "Recovery after recovery": the variable
            // walk's closer consumption may leave a split remainder (an
            // inserted `>` from a shared `>>`) in front of the `=`; skip it
            // so the RHS alias type still attaches.
            if self.peek_kind() != TokenKind::Eq && self.cursor.look_kind(1) == TokenKind::Eq {
                self.cursor.advance();
            }
            if self.peek_kind() == TokenKind::Eq {
                equals = Some(self.peek_span());
                self.cursor.advance();
                let type_info = self.compute_type(true, true);
                // Recovery (:1543-1610): when the RHS type is not a function
                // type, certain follower shapes splice in a missing
                // `Function` — "<type> '(' ... ')';" assumes the type is the
                // return type; "<typeArgs> '(' ... ')';" assumes the '<' opens
                // the function's type parameters. Both report
                // `expectedToken('Function')` on the inserted keyword.
                let type_is_gft = matches!(
                    &type_info,
                    crate::types::TypeInfo::Complex(c) if c.gft_has_return_type.is_some()
                );
                let mut handled = false;
                if matches!(type_info, crate::types::TypeInfo::NoType)
                    && self.peek_kind() == TokenKind::Lt
                    && let Some(end_group) = self.angle_group_after(0)
                {
                    let closer = self.span_of(self.real_id(end_group));
                    match self.kind_at(end_group + 1) {
                        TokenKind::Semicolon => {
                            // Official :1596-1605: `typedef i = <...>;` — the
                            // type arguments belong to a missing `Function`.
                            // The missing parameter list is reported on the
                            // group's closer and the alias parses as
                            // `Function<...>();`.
                            self.report(msg::missing_typedef_parameters(), closer);
                            self.skip_to_token(self.real_id(end_group));
                            self.cursor.advance(); // the closer
                            let open = self.synthetic(TokenKind::OpenParen, closer.range.offset);
                            let close = self.synthetic(TokenKind::CloseParen, closer.range.offset);
                            self.listener.begin_formal_parameters(
                                open,
                                crate::listener::MemberKind::FunctionTypeAlias,
                            );
                            self.listener.end_formal_parameters(
                                0,
                                open,
                                close,
                                crate::listener::MemberKind::FunctionTypeAlias,
                            );
                            self.cursor.advance();
                            self.cursor.advance();
                            handled = true;
                        }
                        TokenKind::OpenParen
                            if let Some(paren_end) =
                                self.cursor.table_matching_end(self.real_id(end_group + 1))
                                && self.kind_at(self.offset_of(paren_end) + 1)
                                    == TokenKind::Semicolon =>
                        {
                            // Official :1564-1610: `<typeArgs>(<params>);` —
                            // insert the missing `Function` before the '<'.
                            self.report(msg::expected_token("Function"), self.peek_span());
                            self.skip_to_token(self.real_id(end_group));
                            self.cursor.advance(); // the '>' closer
                            self.parse_formal_parameters_required_opt(
                                crate::listener::MemberKind::GeneralizedFunctionType,
                            );
                            handled = true;
                        }
                        _ => {}
                    }
                }
                if !handled && !type_is_gft {
                    // Official :1543-1561: `<return type>? '(' ... ')';` —
                    // assume the type is the return type of a missing
                    // `Function`.
                    let head_end = type_info.skip_type(self);
                    let paren = self.offset_of(head_end) + 1;
                    if self.kind_at(paren) == TokenKind::OpenParen
                        && let Some(paren_end) = self.cursor.table_matching_end(self.real_id(paren))
                        && self.kind_at(self.offset_of(paren_end) + 1) == TokenKind::Semicolon
                    {
                        self.report(
                            msg::expected_token("Function"),
                            self.span_of(self.real_id(paren)),
                        );
                        self.skip_to_token(self.real_id(paren));
                        self.parse_formal_parameters_required_opt(
                            crate::listener::MemberKind::GeneralizedFunctionType,
                        );
                        handled = true;
                    }
                }
                if !handled {
                    type_info.ensure_type_or_void(self);
                }
                end = self.ensure_semicolon();
            } else {
                // A rewrite removed the `=`: parse parameters instead.
                self.parse_formal_parameters_required_opt(
                    crate::listener::MemberKind::FunctionTypeAlias,
                );
                end = self.ensure_semicolon();
            }
        } else {
            // Old style: `typedef <returnType> name <typeVars> (params);`
            type_info.parse_type(self);
            if self.peek_kind() == TokenKind::Eq {
                // `typedef void = ...;` — a reserved type head with the `=`
                // at the name position bails the whole typedef: the oracle
                // reports `expectedToken(';')` anchored on the type head and
                // restarts the declaration parse at the `=` (the reparse
                // yields the top-level-member / alias-shape errors).
                let head_end = self
                    .cursor
                    .last_consumed_id()
                    .map_or_else(|| self.peek_span(), |id| self.span_of(id));
                self.report(msg::expected_token(";"), head_end);
                self.listener
                    .end_typedef(augment, typedef_keyword, None, head_end);
                return;
            }
            if self.peek_is_identifier() || self.peek_kind_ahead_is_paren_after_type() {
                let _ = self.ensure_identifier(IdentifierContext::TypedefDeclaration);
            } else {
                // No name at all (`typedef void = ...;`): the 3.13 oracle
                // recovers through the top-level-member message anchored on
                // the offending token before inserting the synthetic name.
                // EOF has no offending token — only the synthetic name.
                if !matches!(self.peek_kind(), TokenKind::Keyword(_) | TokenKind::Eof) {
                    self.report(msg::expected_executable(), self.peek_span());
                }
                let _ = self.ensure_identifier_recovered(IdentifierContext::TypedefDeclaration);
            }
            type_param.parse_variables(self, false);
            self.parse_formal_parameters_required_opt(
                crate::listener::MemberKind::FunctionTypeAlias,
            );
            end = self.ensure_semicolon();
        }
        self.listener
            .end_typedef(augment, typedef_keyword, equals, end);
    }

    fn peek_kind_ahead_is_paren_after_type(&self) -> bool {
        self.cursor.look_kind(0) == TokenKind::OpenParen
    }

    // ------------------------------------------------------------------
    // enum
    // ------------------------------------------------------------------

    /// Official `parseEnum`; the cursor sits on `enum`.
    fn parse_enum(&mut self, begin: TokenSpan, enum_keyword: TokenSpan) {
        let augment = self.pending_augment.take();
        self.listener.begin_enum_declaration_prelude(enum_keyword);
        self.cursor.advance(); // `enum`

        let mut const_keyword: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Const) {
            const_keyword = Some(self.peek_span());
            self.cursor.advance();
        }
        // The 3.13 oracle's enumDeclaration context rejects a reserved head
        // with a missing identifier and a synthetic name (the real token is
        // left for the body recovery below).
        let name = if matches!(self.peek_kind(), TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Reserved)
        {
            let at = self.peek_span();
            self.report(msg::missing_identifier(), at);
            self.synthetic(TokenKind::Identifier, at.range.offset)
        } else {
            self.ensure_identifier(IdentifierContext::EnumDeclaration)
        };
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        type_param.parse_variables(self, true);
        self.listener
            .begin_enum_declaration(begin, augment, enum_keyword, name);
        self.parse_primary_constructor_opt(DeclarationKind::Enum, const_keyword, true);
        self.parse_enum_header_opt(enum_keyword);

        let mut left_brace = self.peek_span();
        let mut member_count = 0usize;
        if self.peek_kind() == TokenKind::Semicolon {
            let semi = self.peek_span();
            self.cursor.advance();
            self.listener
                .handle_enum_header(augment, enum_keyword, semi);
            self.listener.handle_no_enum_body(semi);
            left_brace = semi;
            member_count = 0;
        } else if self.peek_kind() == TokenKind::OpenCurly {
            self.listener
                .handle_enum_header(augment, enum_keyword, left_brace);
            self.listener.begin_enum_body(left_brace);
            self.cursor.advance();
            let mut element_count = 0usize;
            loop {
                if self.is_bailed()
                    || matches!(
                        self.peek_kind(),
                        TokenKind::CloseCurly | TokenKind::Semicolon
                    )
                {
                    break;
                }
                self.parse_enum_element();
                element_count += 1;
                match self.peek_kind() {
                    TokenKind::Comma => {
                        self.cursor.advance();
                    }
                    TokenKind::CloseCurly | TokenKind::Semicolon => break,
                    _ => {
                        // Recovery: identifier suggests a missing comma;
                        // otherwise jump to the group closer.
                        let end_group = self.current_matching_end_for(left_brace);
                        if let Some(close) = end_group
                            && self.cursor.resolve(close).1
                        {
                            self.skip_to_token(close);
                            self.cursor.advance();
                            break;
                        }
                        if is_identifier(self.peek_kind()) {
                            self.report(msg::expected_but_got(","), self.peek_span());
                        } else {
                            self.report(msg::expected_but_got("}"), self.peek_span());
                            if let Some(close) = end_group {
                                self.skip_to_token(close);
                                self.cursor.advance();
                                break;
                            }
                            break;
                        }
                    }
                }
            }
            self.listener
                .handle_enum_elements(self.peek_span(), element_count);
            if self.peek_kind() == TokenKind::Semicolon {
                self.cursor.advance();
                while !self.cursor.at_eof()
                    && !self.is_bailed()
                    && self.peek_kind() != TokenKind::CloseCurly
                {
                    let before = self.cursor.progress();
                    self.parse_class_member_impl(DeclarationKind::Enum, self.enum_name(name));
                    member_count += 1;
                    if self.cursor.progress() == before {
                        // No-progress guard: force-advance past a token no
                        // member parse could consume (official recovery
                        // guarantees termination via the rewriter; here the
                        // guard plays that role).
                        self.report(
                            msg::expected_declaration(self.cursor.lexeme(self.peek_id())),
                            self.peek_span(),
                        );
                        self.force_advance();
                    }
                }
            }
            let close = self.peek_span();
            if self.peek_kind() == TokenKind::CloseCurly {
                self.cursor.advance();
            }
            self.listener.end_enum_body(left_brace, close);
        } else {
            // Recovery: ensure a block.
            let (open, close) = self.ensure_block(msg_block_kind_enum());
            self.listener
                .handle_enum_header(augment, enum_keyword, open);
            self.listener.begin_enum_body(open);
            self.listener.handle_enum_elements(open, 0);
            self.skip_to_token(close.id);
            self.cursor.advance();
            self.listener.end_enum_body(open, close);
            left_brace = open;
        }
        self.listener.end_enum_declaration(
            begin,
            enum_keyword,
            left_brace,
            member_count,
            self.peek_span(),
        );
    }

    fn enum_name(&self, name: TokenSpan) -> Option<&'s str> {
        let lexeme = self.cursor.lexeme(name.id);
        if lexeme.is_empty() {
            None
        } else {
            // The name lexeme lives in the interred source; leak-free via
            // the source lifetime.
            Some(lexeme)
        }
    }

    /// Official `parseEnumHeaderOpt` (`parser_impl.dart:2737-2832`): optional
    /// with-clause, then optional implements-clause, with the bounded gunk
    /// skip (`recoverySmallLookAheadSkipTokens`) and the duplicate/out-of-
    /// order clause recoveries interleaved exactly as the official loop
    /// orders them.
    fn parse_enum_header_opt(&mut self, _enum_keyword: TokenSpan) {
        const LOOK: [TokenKind; 3] = [
            TokenKind::OpenCurly,
            TokenKind::Keyword(Keyword::With),
            TokenKind::Keyword(Keyword::Implements),
        ];
        // Gunk before any clause (:2744-2752).
        if !LOOK.contains(&self.peek_kind()) {
            self.skip_to_clause(&LOOK);
        }
        // With-clause opt (:2754).
        let saw_with = if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
            self.parse_enum_with_clause();
            true
        } else {
            self.listener.handle_enum_no_with_clause();
            false
        };
        // Gunk / duplicate with-clauses before the implements clause
        // (:2758-2773): a `with` here is a duplicate (multipleClauses); other
        // gunk goes through the bounded skip.
        while !self.cursor.at_eof()
            && !matches!(
                self.peek_kind(),
                TokenKind::OpenCurly | TokenKind::Keyword(Keyword::Implements)
            )
        {
            if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
                self.report(msg::multiple_clauses("enum", "with"), self.peek_span());
                self.parse_enum_with_clause_suppressed();
                continue;
            }
            if !self.skip_to_clause(&LOOK) {
                break;
            }
        }
        // Implements-clause opt (:2775).
        if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
            self.parse_implements_opt();
        } else {
            self.listener.handle_implements(None, 0);
        }
        // Out-of-order and duplicate with/implements clauses, plus bounded
        // gunk skips (:2777-2832). The with-kind report is multipleClauses
        // once any with-clause has been seen, outOfOrderClauses before that;
        // an implements token here is always a duplicate.
        let mut has_with: Option<bool> = None;
        while !self.cursor.at_eof() && self.peek_kind() != TokenKind::OpenCurly {
            let hc = *has_with.get_or_insert(saw_with);
            if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
                self.report(
                    if hc {
                        msg::multiple_clauses("enum", "with")
                    } else {
                        msg::out_of_order_clauses("with", "implements")
                    },
                    self.peek_span(),
                );
                self.parse_enum_with_clause_suppressed();
                has_with = Some(true);
            } else if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
                self.report(
                    msg::multiple_clauses("enum", "implements"),
                    self.peek_span(),
                );
                self.parse_implements_opt_suppressed();
            } else if !self.skip_to_clause(&LOOK) {
                break;
            }
        }
    }

    fn parse_enum_with_clause(&mut self) {
        let with = self.peek_span();
        self.cursor.advance();
        self.parse_type_list();
        self.listener.handle_enum_with_clause(with);
    }

    /// Duplicate with-clause: report done by the caller; the types parse
    /// silently (official swaps in the `NullListener`; we skip without events).
    fn parse_enum_with_clause_suppressed(&mut self) {
        self.cursor.advance(); // `with`
        self.skip_type_list_raw();
    }

    fn parse_implements_opt(&mut self) {
        let implements = self.peek_span();
        self.cursor.advance();
        let mut count = 1usize;
        self.parse_single_type_required();
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.parse_single_type_required();
            count += 1;
        }
        self.listener.handle_implements(Some(implements), count);
    }

    fn parse_implements_opt_suppressed(&mut self) {
        self.cursor.advance(); // `implements`
        self.skip_single_type_raw();
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.skip_single_type_raw();
        }
    }

    /// Official `parseEnumElement`.
    fn parse_enum_element(&mut self) {
        let begin = self.peek_span();
        self.parse_metadata_star();
        let augment = self.pending_augment.take();

        self.ensure_identifier(IdentifierContext::EnumValueDeclaration);
        let mut has_type_arguments_or_dot = false;
        {
            let start = self.peek_span();
            self.listener
                .handle_no_type_name_in_constructor_reference(start);
            self.listener.begin_constructor_reference(start);
            let type_arg = self.compute_type_param_or_arg_on_lt(false);
            if !matches!(type_arg, TypeParamOrArg::None) {
                has_type_arguments_or_dot = true;
            }
            type_arg.parse_arguments(self);
            let mut period = None;
            if self.peek_kind() == TokenKind::Period {
                has_type_arguments_or_dot = true;
                period = Some(self.peek_span());
                self.cursor.advance();
                self.ensure_identifier(
                    IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments,
                );
            } else {
                let at = self.peek_span();
                self.listener
                    .handle_no_constructor_reference_continuation_after_type_arguments(at);
            }
            self.listener.end_constructor_reference(
                start,
                period,
                self.peek_span(),
                ConstructorReferenceContext::Const,
            );
        }
        if self.peek_kind() == TokenKind::OpenParen || has_type_arguments_or_dot {
            // Official `parseConstructorInvocationArguments` (the enum
            // element is a verbatim copy of the constructor-reference tail,
            // parser_impl.dart:2948-2950): the missing-`(` recovery reports
            // `expected_after_but_got('(')` and splices an empty synthetic
            // group instead of eating the `;`/`,` follower.
            self.parse_constructor_invocation_arguments(self.last_consumed_span());
        } else {
            let at = self.peek_span();
            self.listener.handle_no_arguments(at);
        }
        self.listener.handle_enum_element(begin, augment);
    }

    // ------------------------------------------------------------------
    // class / named mixin application
    // ------------------------------------------------------------------

    /// Official `parseClassOrNamedMixinApplication`; the cursor sits on
    /// `class` (the modifiers were consumed by the dispatch walk).
    fn parse_class_or_named_mixin_application(
        &mut self,
        begin: TokenSpan,
        abstract_kw: Option<TokenSpan>,
        sealed: Option<TokenSpan>,
        base: Option<TokenSpan>,
        interface: Option<TokenSpan>,
        final_kw: Option<TokenSpan>,
        augment: Option<TokenSpan>,
        mixin: Option<TokenSpan>,
        class_keyword: TokenSpan,
    ) {
        self.listener
            .begin_class_or_mixin_or_named_mixin_application_prelude(begin);
        self.cursor.advance(); // `class`

        let mut const_keyword: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Const) {
            const_keyword = Some(self.peek_span());
            self.cursor.advance();
        }
        let name = self.ensure_identifier(IdentifierContext::ClassOrMixinOrExtensionDeclaration);
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        type_param.parse_variables(self, true);

        // The abstract+final/interface/sealed modifier trio is owned by the
        // semantic layer (dartq-resolution modifiers.rs, check_modifier_
        // declaration) — the only emitter that can carry corrections. The
        // parser must not re-report it.

        if self.peek_kind() == TokenKind::Eq {
            if let Some(c) = const_keyword {
                self.report(msg::const_without_primary_constructor(), c);
            }
            if let Some(a) = augment {
                self.report(msg::mixin_application_class_augmentation(), a);
            }
            self.listener.begin_named_mixin_application(
                begin,
                abstract_kw,
                sealed,
                base,
                interface,
                final_kw,
                augment,
                mixin,
                name,
            );
            self.parse_named_mixin_application(begin, class_keyword);
        } else {
            self.listener.begin_class_declaration(
                begin,
                abstract_kw,
                sealed,
                base,
                interface,
                final_kw,
                augment,
                mixin,
                name,
            );
            self.parse_class(begin, class_keyword, const_keyword, name);
        }
    }

    /// Official `parseNamedMixinApplication`.
    ///
    /// The implements clause emits the real `handleImplements` event (the
    /// official diet parser folds it into `endNamedMixinApplication`; the
    /// dartq event stream carries clause names through `handle_implements`
    /// so the semantic collector attributes them to the implements face
    /// instead of the header residue).
    fn parse_named_mixin_application(&mut self, begin: TokenSpan, class_keyword: TokenSpan) {
        let equals = self.peek_span();
        self.cursor.advance();
        let type_info = self.compute_type(true, false);
        type_info.ensure_type_not_void(self);
        self.parse_mixin_application_rest();
        let mut implements = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
            implements = Some(self.peek_span());
            self.cursor.advance();
            let mut count = 1usize;
            self.parse_single_type_required();
            while self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                self.parse_single_type_required();
                count += 1;
            }
            self.listener.handle_implements(implements, count);
        } else {
            self.listener.handle_implements(None, 0);
        }
        let end = self.ensure_semicolon();
        self.listener
            .end_named_mixin_application(begin, class_keyword, equals, implements, end);
    }

    /// Official `parseMixinApplicationRest` (`with A, B`).
    fn parse_mixin_application_rest(&mut self) {
        let with = self.peek_span();
        if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
            self.cursor.advance();
        } else {
            self.report(msg::expected_but_got("with"), with);
        }
        self.parse_type_list();
        self.listener
            .handle_named_mixin_application_with_clause(with);
    }

    /// Official `parseTypeList`: `type (',' type)*`.
    fn parse_type_list(&mut self) {
        let begin = self.peek_span();
        self.listener.begin_type_list(begin);
        let mut count = 1usize;
        self.parse_single_type_required();
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.parse_single_type_required();
            count += 1;
        }
        self.listener.end_type_list(count);
    }

    /// One required type (`computeType(required: true).ensureTypeNotVoid`).
    fn parse_single_type_required(&mut self) {
        let type_info = self.compute_type(true, false);
        type_info.ensure_type_not_void(self);
    }

    /// Raw single-type skip (no events; the suppressed-parse stand-in).
    fn skip_single_type_raw(&mut self) {
        let type_info = self.compute_type(true, false);
        type_info.skip_type_raw(self);
    }

    /// Raw `,`-separated type-list skip (no events).
    fn skip_type_list_raw(&mut self) {
        self.skip_single_type_raw();
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.skip_single_type_raw();
        }
    }

    /// Official `parseClass`; the cursor sits after the type variables.
    fn parse_class(
        &mut self,
        begin: TokenSpan,
        class_keyword: TokenSpan,
        const_keyword: Option<TokenSpan>,
        name: TokenSpan,
    ) {
        if self.parse_primary_constructor_opt(DeclarationKind::Class, const_keyword, true) {
            let header = self.parse_class_header_opt(class_keyword, begin);

            if self.peek_kind() == TokenKind::Semicolon {
                let semi = self.peek_span();
                self.cursor.advance();
                if !self.features.primary_constructors_enabled {
                    self.report_experiment_not_enabled(semi);
                }
                self.listener.handle_no_class_body(semi);
            } else {
                if self.peek_kind() != TokenKind::OpenCurly {
                    // Recovery: out-of-order/duplicate clauses (the missing body
                    // itself is ensured once, by the body parser below — official
                    // `parseClassOrMixinOrExtensionBodyImpl` only asserts the `{`
                    // its caller's `ensureBlock` guaranteed, so the class header's
                    // own `ensureBlock` report and the body's must not stack).
                    self.parse_class_header_recovery(DeclarationHeaderKind::Class, header);
                }
                self.parse_class_or_mixin_or_extension_body(DeclarationKind::Class, name);
            }
        } else {
            // The feature-off `(` head recovery consumed the tail; the
            // declaration still ends with a balanced empty body. The queued
            // synthetic braces must be drained after their events: a full
            // inserted queue would resurface at the unit loop as a phantom
            // declaration and parse as a bogus member (the same
            // drain-after-insert contract as `ensureSemicolon`,
            // recovery.rs `while self.cursor.advance() != id {}`).
            let offset = self.peek_span().range.offset;
            let open = self.synthetic(TokenKind::OpenCurly, offset);
            let close = self.synthetic(TokenKind::CloseCurly, offset);
            self.listener
                .begin_class_or_mixin_or_extension_body(DeclarationKind::Class, open);
            self.listener.end_class_or_mixin_or_extension_body(
                DeclarationKind::Class,
                0,
                open,
                close,
            );
            while self.cursor.advance() != close.id {}
        }
        let end = self.peek_span();
        self.listener.end_class_declaration(begin, end);
    }

    /// Official `parseClassHeaderOpt`. Returns the clauses seen. `class_kw`
    /// is the `class`/`mixin class` keyword for the header event.
    fn parse_class_header_opt(&mut self, class_kw: TokenSpan, begin: TokenSpan) -> HeaderClauses {
        let mut clauses = HeaderClauses::default();
        // extends
        if self.peek_kind() == TokenKind::Keyword(Keyword::Extends) {
            let extends_kw = self.peek_span();
            self.cursor.advance();
            self.parse_class_extends_seen(extends_kw, DeclarationHeaderKind::Class);
            clauses.extends = true;
        } else {
            let at = self.peek_span();
            self.listener.handle_no_type(at);
            self.listener.handle_class_extends(None, 1);
        }
        // with
        if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
            let with = self.peek_span();
            self.cursor.advance();
            self.parse_type_list();
            self.listener.handle_class_with_clause(with);
            clauses.with = true;
        } else {
            self.listener.handle_class_no_with_clause();
        }
        // implements
        clauses.implements = self.parse_implements_clause();
        // native clause (deprecated)
        let mut native = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Native) {
            let native_kw = self.peek_span();
            self.cursor.advance();
            let has_name = self.peek_kind() == TokenKind::String;
            if has_name {
                self.parse_literal_string();
            }
            self.listener.handle_native_clause(native_kw, has_name);
            self.report(msg::native_clause_should_be_annotation(), native_kw);
            native = Some(native_kw);
        }
        self.listener.handle_class_header(begin, class_kw, native);
        clauses
    }

    /// `parseClassOrMixinOrEnumImplementsOpt` with the event.
    fn parse_implements_clause(&mut self) -> bool {
        if self.peek_kind() != TokenKind::Keyword(Keyword::Implements) {
            self.listener.handle_implements(None, 0);
            return false;
        }
        let implements = self.peek_span();
        self.cursor.advance();
        let mut count = 1usize;
        self.parse_single_type_required();
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.parse_single_type_required();
            count += 1;
        }
        self.listener.handle_implements(Some(implements), count);
        true
    }

    /// Official `parseClassExtendsSeenExtendsClause`.
    fn parse_class_extends_seen(&mut self, extends_kw: TokenSpan, kind: DeclarationHeaderKind) {
        let type_info = self.compute_type(true, false);
        type_info.ensure_type_not_void(self);
        let mut count = 1usize;
        if self.peek_kind() == TokenKind::Comma {
            if kind == DeclarationHeaderKind::Class {
                self.report(msg::multiple_extends(), self.peek_span());
            }
            while self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                let type_info = self.compute_type(true, false);
                type_info.ensure_type_not_void(self);
                count += 1;
            }
        }
        self.listener.handle_class_extends(Some(extends_kw), count);
    }

    /// Official `parseDeclarationHeaderRecoveryInternal` (mechanism note: the
    /// official re-parses the header under a swallowing recovery listener to
    /// learn the already-seen clauses; here the first pass reports what it
    /// saw directly, so the recovery loop only handles the remaining tokens).
    fn parse_class_header_recovery(&mut self, kind: DeclarationHeaderKind, seen: HeaderClauses) {
        let mut clauses = seen;
        loop {
            if self.is_bailed() {
                break;
            }
            let before = self.cursor.progress();
            // skipUnexpectedTokenOpt: a non-keyword token directly before a
            // clause keyword is reported and skipped.
            let next = self.peek_kind();
            if !matches!(next, TokenKind::Keyword(_)) {
                let follower = self.cursor.look_kind(1);
                if matches!(
                    follower,
                    TokenKind::Keyword(Keyword::Extends | Keyword::With | Keyword::Implements)
                ) || follower == TokenKind::OpenCurly
                {
                    self.report(
                        msg::unexpected_token(self.cursor.lexeme(self.peek_id())),
                        self.peek_span(),
                    );
                    self.cursor.advance();
                }
            }
            // extends (`extend`/`on` read as a misspelled `extends` for every
            // declaration kind; the official gate is lexeme-based over
            // `isKeywordOrIdentifier` (:3218), so a plain identifier
            // `extend` qualifies; ExtensionType additionally rejects the
            // clause)
            if is_keyword_or_identifier(self.peek_kind())
                && matches!(self.cursor.lexeme(self.peek_id()), "extend" | "on")
            {
                let misspelled = self.peek_span();
                self.report(msg::expected_instead("extends"), misspelled);
                self.cursor.advance();
                self.parse_class_extends_seen(misspelled, kind);
                if kind == DeclarationHeaderKind::ExtensionType {
                    self.report(msg::extension_type_extends(), misspelled);
                } else {
                    clauses.extends = true;
                }
            } else if self.peek_kind() == TokenKind::Keyword(Keyword::Extends) {
                let extends_kw = self.peek_span();
                self.cursor.advance();
                self.parse_class_extends_seen(extends_kw, kind);
                if kind == DeclarationHeaderKind::ExtensionType {
                    self.report(msg::extension_type_extends(), extends_kw);
                } else if clauses.extends {
                    self.report(msg::multiple_extends(), extends_kw);
                } else {
                    if clauses.with {
                        self.report(msg::with_before_extends(), extends_kw);
                    } else if clauses.implements {
                        self.report(msg::implements_before_extends(), extends_kw);
                    }
                    clauses.extends = true;
                }
            }
            // with
            if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
                let with = self.peek_span();
                self.cursor.advance();
                self.parse_type_list();
                self.listener.handle_class_with_clause(with);
                if kind == DeclarationHeaderKind::ExtensionType {
                    self.report(msg::extension_type_with(), with);
                } else if clauses.with {
                    self.report(msg::multiple_with_clauses(), with);
                } else {
                    if clauses.implements {
                        self.report(msg::implements_before_with(), with);
                    }
                    clauses.with = true;
                }
            }
            // implements
            if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
                let implements = self.peek_span();
                self.cursor.advance();
                let mut count = 1usize;
                self.parse_single_type_required();
                while self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    self.parse_single_type_required();
                    count += 1;
                }
                self.listener.handle_implements(Some(implements), count);
                if clauses.implements {
                    self.report(msg::multiple_implements(), implements);
                } else {
                    clauses.implements = true;
                }
            }
            self.listener.handle_recover_declaration_header(kind);
            if self.peek_kind() == TokenKind::OpenCurly || self.no_progress(before) {
                break;
            }
        }
    }

    /// Class/mixin/extension/extension-type body (official
    /// `parseClassOrMixinOrExtensionBody`).
    fn parse_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, name: TokenSpan) {
        let (open, close) = self.ensure_block(match kind {
            DeclarationKind::Class => msg_block_kind_class(),
            DeclarationKind::Mixin => msg_block_kind_mixin(),
            DeclarationKind::Enum => msg_block_kind_enum(),
            DeclarationKind::Extension => msg_block_kind_extension(),
            DeclarationKind::ExtensionType => msg_block_kind_extension_type(),
            DeclarationKind::TopLevel => unreachable!("top-level has no body"),
        });
        self.cursor.advance(); // `{` (real) — for synthetic blocks the
        // queued `{` is consumed by the same advance.
        let enclosing_name = self.cursor.lexeme(name.id);
        let enclosing = if enclosing_name.is_empty() {
            None
        } else {
            Some(enclosing_name)
        };
        self.listener
            .begin_class_or_mixin_or_extension_body(kind, open);
        let mut count = 0usize;
        while !self.cursor.at_eof()
            && !self.is_bailed()
            && self.peek_kind() != TokenKind::CloseCurly
        {
            let before = self.cursor.progress();
            self.parse_class_member_impl(kind, enclosing);
            count += 1;
            if self.no_progress(before) {
                // Recovery guarantee: every member consumes at least one
                // token (official per-loop no-progress guards).
                self.force_advance();
            }
        }
        self.skip_to_token(close.id);
        let end = self.peek_span();
        self.cursor.advance();
        self.listener
            .end_class_or_mixin_or_extension_body(kind, count, open, end);
    }

    // ------------------------------------------------------------------
    // mixin
    // ------------------------------------------------------------------

    /// Official `parseMixin`; the cursor sits on `mixin`.
    fn parse_mixin(
        &mut self,
        begin: TokenSpan,
        augment: Option<TokenSpan>,
        base: Option<TokenSpan>,
        mixin_keyword: TokenSpan,
    ) {
        self.listener
            .begin_class_or_mixin_or_named_mixin_application_prelude(mixin_keyword);
        self.cursor.advance(); // `mixin`
        let mut const_keyword: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Const) {
            const_keyword = Some(self.peek_span());
            self.cursor.advance();
        }
        let name = self.ensure_identifier(IdentifierContext::ClassOrMixinOrExtensionDeclaration);
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        type_param.parse_variables(self, true);
        self.listener
            .begin_mixin_declaration(begin, augment, base, mixin_keyword, name);

        // header: primary constructor (reported), on, implements.
        self.parse_primary_constructor_opt(DeclarationKind::Mixin, const_keyword, true);
        let mut clauses = HeaderClauses::default();
        // on
        if self.peek_kind() == TokenKind::Keyword(Keyword::On) {
            let on = self.peek_span();
            self.cursor.advance();
            if augment.is_some() {
                self.report(msg::mixin_augmentation_has_on_clause(), on);
            }
            let mut count = 1usize;
            self.parse_single_type_required();
            while self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                self.parse_single_type_required();
                count += 1;
            }
            self.listener.handle_mixin_on(Some(on), count);
            clauses.extends = true; // reuse: `on` seen
        } else {
            self.listener.handle_mixin_on(None, 0);
        }
        clauses.implements = self.parse_implements_clause();
        self.listener.handle_mixin_header(mixin_keyword);

        if self.peek_kind() == TokenKind::Semicolon {
            let semi = self.peek_span();
            self.cursor.advance();
            if !self.features.primary_constructors_enabled {
                self.report_experiment_not_enabled(semi);
            }
            self.listener.handle_no_mixin_body(semi);
        } else {
            if self.peek_kind() != TokenKind::OpenCurly {
                self.parse_mixin_header_recovery(clauses, augment.is_some());
            }
            self.parse_class_or_mixin_or_extension_body(DeclarationKind::Mixin, name);
        }
        let end = self.peek_span();
        self.listener.end_mixin_declaration(begin, end);
    }

    /// Official `parseMixinHeaderRecovery` (same mechanism note as the class
    /// header recovery).
    fn parse_mixin_header_recovery(&mut self, seen: HeaderClauses, is_augmentation: bool) {
        let mut clauses = seen;
        loop {
            if self.is_bailed() {
                break;
            }
            let before = self.cursor.progress();
            // on
            if self.peek_kind() == TokenKind::Keyword(Keyword::On) {
                let on = self.peek_span();
                self.cursor.advance();
                let mut count = 1usize;
                self.parse_single_type_required();
                while self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    self.parse_single_type_required();
                    count += 1;
                }
                self.listener.handle_mixin_on(Some(on), count);
                if clauses.extends {
                    self.report(msg::multiple_on_clauses(), on);
                } else {
                    if clauses.implements {
                        self.report(msg::implements_before_on(), on);
                    }
                    clauses.extends = true;
                }
            } else if is_keyword_or_identifier(self.peek_kind())
                && matches!(self.cursor.lexeme(self.peek_id()), "extend" | "extends")
            {
                // Official :3519 — same lexeme-based gate as the class
                // recovery; `extend` is an identifier, not a keyword.
                self.report(msg::expected_instead("on"), self.peek_span());
                self.cursor.advance();
                let mut count = 1usize;
                self.parse_single_type_required();
                while self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    self.parse_single_type_required();
                    count += 1;
                }
                self.listener.handle_mixin_on(Some(self.peek_span()), count);
            }
            // implements
            if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
                let implements = self.peek_span();
                self.cursor.advance();
                let mut count = 1usize;
                self.parse_single_type_required();
                while self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    self.parse_single_type_required();
                    count += 1;
                }
                self.listener.handle_implements(Some(implements), count);
                if clauses.implements {
                    self.report(msg::multiple_implements(), implements);
                } else {
                    clauses.implements = true;
                }
            }
            // with (always invalid on a mixin)
            if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
                let with = self.peek_span();
                self.cursor.advance();
                self.report(msg::mixin_with_clause(), with);
                self.parse_type_list();
                self.listener.handle_mixin_with_clause(with);
            }
            self.listener.handle_recover_mixin_header();
            let _ = is_augmentation;
            if self.peek_kind() == TokenKind::OpenCurly || self.no_progress(before) {
                break;
            }
        }
    }

    // ------------------------------------------------------------------
    // extension / extension type
    // ------------------------------------------------------------------

    /// Official `parseExtension`; the cursor sits on `extension`.
    fn parse_extension(
        &mut self,
        begin: TokenSpan,
        augment: Option<TokenSpan>,
        extension_keyword: TokenSpan,
    ) {
        self.listener
            .begin_extension_declaration_prelude(extension_keyword);
        // Consume `extension` (official `parseExtension` reads `token.next`
        // from here on, so both sub-parsers start after the keyword).
        self.cursor.advance();
        if self.peek_kind() == TokenKind::Identifier && self.cursor.lexeme(self.peek_id()) == "type"
        {
            let type_keyword = self.peek_span();
            self.cursor.advance();
            self.parse_extension_type_declaration(begin, augment, extension_keyword, type_keyword);
        } else {
            self.parse_extension_declaration(begin, augment, extension_keyword);
        }
    }

    /// Official `parseExtensionDeclaration`; the cursor sits after
    /// `extension`.
    fn parse_extension_declaration(
        &mut self,
        begin: TokenSpan,
        augment: Option<TokenSpan>,
        extension_keyword: TokenSpan,
    ) {
        let mut const_keyword: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Const) {
            const_keyword = Some(self.peek_span());
            self.cursor.advance();
        }
        let mut name: Option<TokenSpan> = None;
        if self.peek_is_identifier() && self.peek_kind() != TokenKind::Keyword(Keyword::On) {
            if is_builtin_identifier(self.peek_kind()) {
                self.report(
                    msg::built_in_identifier_in_declaration(self.cursor.lexeme(self.peek_id())),
                    self.peek_span(),
                );
            }
            name = Some(self.peek_span());
            self.cursor.advance();
        } else if let Some(a) = augment {
            self.report(msg::extension_augmentation_without_name(), a);
        }
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        type_param.parse_variables(self, false);
        self.listener
            .begin_extension_declaration(augment, extension_keyword, name);
        self.parse_primary_constructor_opt(DeclarationKind::Extension, const_keyword, true);

        let mut on_keyword: Option<TokenSpan> = Some(self.peek_span());
        if augment.is_some() {
            if self.peek_kind() == TokenKind::Keyword(Keyword::On) {
                self.report(
                    msg::extension_augmentation_has_on_clause(),
                    self.peek_span(),
                );
                self.cursor.advance();
                let type_info = self.compute_type(true, false);
                type_info.ensure_type_or_void(self);
            } else {
                on_keyword = None;
            }
        } else if self.peek_kind() == TokenKind::Keyword(Keyword::On) {
            on_keyword = Some(self.peek_span());
            self.cursor.advance();
            let type_info = self.compute_type(true, false);
            type_info.ensure_type_or_void(self);
        } else {
            let at = self.peek_span();
            if matches!(
                self.peek_kind(),
                TokenKind::Keyword(Keyword::Extends | Keyword::Implements | Keyword::With)
            ) {
                self.report(msg::expected_instead("on"), at);
            } else {
                self.report(msg::expected_after_but_got("on"), at);
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Keyword(Keyword::On), at.range.offset);
                self.cursor.advance();
                on_keyword = Some(self.span_of(id));
            }
            let type_info = self.compute_type(true, false);
            type_info.ensure_type_or_void(self);
        }

        if self.peek_kind() == TokenKind::Semicolon {
            let semi = self.peek_span();
            self.cursor.advance();
            if !self.features.primary_constructors_enabled {
                self.report_experiment_not_enabled(semi);
            }
            self.listener.handle_no_extension_body(semi);
        } else {
            if self.peek_kind() != TokenKind::OpenCurly {
                // Recovery: skip stray `, extends/implements/on/with [id]`
                // runs (official parseExtensionDeclaration recovery).
                loop {
                    let kind = self.peek_kind();
                    if matches!(
                        kind,
                        TokenKind::Comma
                            | TokenKind::Keyword(
                                Keyword::Extends
                                    | Keyword::Implements
                                    | Keyword::On
                                    | Keyword::With
                            )
                    ) {
                        self.report(
                            msg::unexpected_token(self.cursor.lexeme(self.peek_id())),
                            self.peek_span(),
                        );
                        self.cursor.advance();
                        if is_identifier(self.peek_kind()) {
                            self.cursor.advance();
                        }
                    } else {
                        break;
                    }
                }
            }
            self.parse_class_or_mixin_or_extension_body(
                DeclarationKind::Extension,
                name.unwrap_or(extension_keyword),
            );
        }
        let end = self.peek_span();
        self.listener
            .end_extension_declaration(begin, extension_keyword, on_keyword, end);
    }

    /// Official `parseExtensionTypeDeclaration`; the cursor sits after the
    /// `type` keyword.
    fn parse_extension_type_declaration(
        &mut self,
        begin: TokenSpan,
        augment: Option<TokenSpan>,
        extension_keyword: TokenSpan,
        type_keyword: TokenSpan,
    ) {
        let mut const_keyword: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Const) {
            const_keyword = Some(self.peek_span());
            self.cursor.advance();
        }
        let name = if self.peek_is_identifier() {
            if is_builtin_identifier(self.peek_kind()) {
                self.report(
                    msg::built_in_identifier_in_declaration(self.cursor.lexeme(self.peek_id())),
                    self.peek_span(),
                );
            }
            let n = self.peek_span();
            self.cursor.advance();
            n
        } else {
            self.ensure_identifier(IdentifierContext::ClassOrMixinOrExtensionDeclaration)
        };
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        type_param.parse_variables(self, false);
        self.listener
            .begin_extension_type_declaration(augment, extension_keyword, name);
        self.parse_primary_constructor_opt(
            DeclarationKind::ExtensionType,
            const_keyword,
            augment.is_none(),
        );

        let implements_seen = self.parse_implements_clause();
        let _ = implements_seen;

        if self.peek_kind() == TokenKind::Semicolon {
            let semi = self.peek_span();
            self.cursor.advance();
            if !self.features.primary_constructors_enabled {
                self.report_experiment_not_enabled(semi);
            }
            self.listener.handle_no_extension_type_body(semi);
        } else {
            if self.peek_kind() != TokenKind::OpenCurly {
                // The implements clause parsed above counts toward the
                // at-most-one rule the recovery loop enforces
                // (`parseDeclarationHeaderRecoveryInternal` re-parses the
                // header to learn exactly this).
                let seen = HeaderClauses {
                    implements: implements_seen,
                    ..HeaderClauses::default()
                };
                self.parse_class_header_recovery(DeclarationHeaderKind::ExtensionType, seen);
            }
            self.parse_class_or_mixin_or_extension_body(DeclarationKind::ExtensionType, name);
        }
        let end = self.peek_span();
        self.listener.end_extension_type_declaration(
            begin,
            augment,
            extension_keyword,
            type_keyword,
            end,
        );
    }

    /// Official `parsePrimaryConstructorOpt`. Returns `true` when the caller
    /// must continue with the header/body tail, `false` when this function
    /// already consumed the whole declaration tail (feature-off `(` recovery
    /// below).
    fn parse_primary_constructor_opt(
        &mut self,
        kind: DeclarationKind,
        const_keyword: Option<TokenSpan>,
        allow_representation: bool,
    ) -> bool {
        if kind == DeclarationKind::Class
            && !self.features.primary_constructors_enabled
            && self.peek_kind() == TokenKind::OpenParen
        {
            return self.recover_class_head_paren_off();
        }
        if self.peek_kind() == TokenKind::OpenParen || self.peek_kind() == TokenKind::Period {
            let begin = self.peek_span();
            match kind {
                DeclarationKind::Mixin => {
                    if self.features.primary_constructors_enabled {
                        self.report(
                            msg::mixin_primary_constructor(),
                            const_keyword.unwrap_or(begin),
                        );
                    } else {
                        self.report(
                            msg::unexpected_token(self.cursor.lexeme(begin.id)),
                            const_keyword.unwrap_or(begin),
                        );
                    }
                }
                DeclarationKind::Extension => {
                    if self.features.primary_constructors_enabled {
                        self.report(
                            msg::extension_primary_constructor(),
                            const_keyword.unwrap_or(begin),
                        );
                    } else {
                        self.report(
                            msg::unexpected_token(self.cursor.lexeme(begin.id)),
                            const_keyword.unwrap_or(begin),
                        );
                    }
                }
                DeclarationKind::ExtensionType if !allow_representation => {
                    self.report(
                        msg::extension_type_augmentation_specifies_representation_field(),
                        const_keyword.unwrap_or(begin),
                    );
                }
                _ => {}
            }

            self.listener.begin_primary_constructor(begin);
            let has_constructor_name = self.peek_kind() == TokenKind::Period;
            if has_constructor_name {
                self.cursor.advance();
                self.ensure_identifier(IdentifierContext::PrimaryConstructorDeclaration);
            }
            if self.peek_kind() == TokenKind::OpenParen {
                if kind == DeclarationKind::ExtensionType && allow_representation {
                    self.rep_field_check = Some(crate::parser::RepFieldCheck::default());
                }
                self.parse_formal_parameters_required_opt(
                    crate::listener::MemberKind::PrimaryConstructor,
                );
                let rep = self.rep_field_check.take();
                if let Some(state) = rep {
                    // `_checkForExtensionTypeRepresentationErrorCodes`
                    // (error_verifier.dart:5356-5460; primary constructors
                    // feature disabled arm).
                    if state.count == 0 {
                        self.report(msg::expected_representation_field(), self.peek_span());
                    } else if state.count > 1 {
                        if let Some(comma) = state.comma_after_first {
                            self.report(msg::multiple_representation_fields(), comma);
                        }
                    } else if state.this_or_super {
                        // Reported at the this/super keyword; the keyword span
                        // is not carried in the state, so anchor at the
                        // parameter's modifier or the formals begin.
                        self.report(
                            msg::expected_representation_field(),
                            state.modifier.unwrap_or(begin),
                        );
                    } else if let Some(delim) = state.delimiter {
                        self.report(msg::expected_representation_field(), delim);
                    } else if state.fn_typed {
                        // The verifier's `expectedRepresentationField` for a
                        // function-typed representation field only runs with
                        // primary constructors disabled (the 3.13 oracle runs
                        // the feature and accepts the declaring parameter).
                        if !self.features.primary_constructors_enabled {
                            self.report(msg::expected_representation_field(), begin);
                        }
                    } else if let Some(m) = state.modifier {
                        let lexeme = self.cursor.lexeme(m.id);
                        if !self.features.primary_constructors_enabled || lexeme == "var" {
                            self.report(msg::representation_field_modifier(lexeme), m);
                        }
                    }
                }
            } else {
                let report_missing_parameters =
                    matches!(kind, DeclarationKind::Class | DeclarationKind::Enum)
                        || (kind == DeclarationKind::ExtensionType && allow_representation);
                if report_missing_parameters {
                    self.report(
                        msg::missing_primary_constructor_parameters(),
                        self.peek_span(),
                    );
                }
                if kind == DeclarationKind::ExtensionType {
                    // `_checkForExtensionTypeRepresentationErrorCodes`: the
                    // synthesized empty formal list has no representation
                    // field (error_verifier.dart:5366).
                    self.report(msg::expected_representation_field(), self.peek_span());
                }
                let at = self.peek_span();
                self.listener.handle_no_formal_parameters(
                    at,
                    crate::listener::MemberKind::PrimaryConstructor,
                );
            }
            let end = self.peek_span();
            self.listener.end_primary_constructor(
                kind,
                begin,
                end,
                const_keyword,
                has_constructor_name,
            );
        } else {
            if kind == DeclarationKind::ExtensionType {
                // The 3.13 oracle reports the missing primary constructor for
                // every extension type without a representation — including
                // augmentations (no augmentation gating there) — and the
                // verifier's synthesized empty formal list additionally yields
                // `expectedRepresentationField`. The report anchors on the
                // parser pointer (official :3876 `token`, the last consumed
                // token — the declaration name when the `{` follows it).
                self.report(
                    msg::missing_primary_constructor(),
                    self.last_consumed_anchor(),
                );
                self.report(msg::expected_representation_field(), self.peek_span());
            } else if let Some(c) = const_keyword {
                match kind {
                    DeclarationKind::Mixin => self.report(msg::mixin_primary_constructor(), c),
                    DeclarationKind::Extension => {
                        self.report(msg::extension_primary_constructor(), c);
                    }
                    _ => self.report(msg::const_without_primary_constructor(), c),
                }
            }
            let at = self.peek_span();
            self.listener
                .handle_no_primary_constructor(kind, at, const_keyword);
        }
        true
    }

    /// Feature-off `(` face at a class head, probe-verified against the
    /// 3.13.1 oracle at its default language version (primary constructors
    /// disabled): the `(` is not a parameter list there — 3.15 main parses
    /// class-head formals unconditionally (`parser_impl.dart:3782-3838`),
    /// which our feature-on path mirrors. For `class C( {}` the oracle's
    /// full ordered face set is `expected_token(';')` at the `{`,
    /// `missing_identifier` + `missing_const_final_var_or_type` at the `}`
    /// (the empty `{}`-remnant parses as a member with no type and no
    /// name), then `expected_executable` at the pending paren's EOF
    /// position; the `)` face itself is carried by the scanner's
    /// unbalanced-group diagnostic (`lexer/mod.rs:908-917`), not by the
    /// parser. With a real follower token instead of EOF, the oracle
    /// consumes the follower's keyword as a junk member inside this class:
    /// the `(` surfaces as skipped junk (`unexpected_token` on the skipped
    /// token, `parser_impl.dart:2889`) and the boundary token carries the
    /// `expected_executable` junk-member face. Returns `false`: the tail
    /// was consumed.
    fn recover_class_head_paren_off(&mut self) -> bool {
        if self.cursor.look_kind(1) != TokenKind::OpenCurly {
            // Not the probed shape: keep the regular header/body path.
            return true;
        }
        let lparen = self.peek_span();
        let lparen_lexeme = self.cursor.lexeme(lparen.id);
        self.cursor.advance(); // `(` — silent on the EOF path (see tail)
        let open = self.peek_span();
        self.report(msg::expected_token(";"), open);
        let group_close = self.current_matching_end();
        self.cursor.advance(); // `{` consumed as the junk head boundary
        if self.peek_kind() == TokenKind::CloseCurly {
            // Empty `{}`-remnant: a member with no type and no name.
            let close = self.peek_span();
            self.report(msg::missing_identifier(), close);
            self.report(msg::missing_const_final_var_or_type(), close);
            self.cursor.advance();
        } else if let Some(close_id) = group_close {
            // Non-empty group: bounded walk to the scanner-paired `}`.
            self.skip_to_token(close_id);
            self.cursor.advance();
        } else {
            // Unpaired `{`: walk to EOF (or the first stray `}`).
            while !self.cursor.at_eof() && self.peek_kind() != TokenKind::CloseCurly {
                self.cursor.advance();
            }
            if self.peek_kind() == TokenKind::CloseCurly {
                self.cursor.advance();
            }
        }
        // The pending `(` closes wherever the recovery can bound it, and the
        // two oracle probes split exactly there. EOF path (`class C( {}`):
        // the scanner reports the unbalanced group itself
        // (`expected_token(')')` pinned at EOF, lexer/mod.rs:908-917) and
        // appends a zero-width synthetic `)` before the EOF token
        // (lexer/mod.rs:140-141); the oracle anchors its missing-member face
        // at that same spot as `expected_executable`, and the closer is
        // consumed — together with any residual recovery synthetics — so
        // the top-level junk path cannot re-parse it. Follower path
        // (`class C( {}` + a real next token): the oracle consumes the
        // follower's keyword as a junk member inside this class — the head
        // `(` surfaces as skipped junk (`unexpectedToken` anchored on the
        // skipped token, parser_impl.dart:2889), and the boundary token
        // carries the junk-member face (`expected_executable` spans it);
        // the paren-close `)` row is then covered by the scanner's EOF
        // diagnostic (the (code, file) multiset is identical). The follower
        // declaration itself keeps its own clean parse — observably
        // identical to the oracle's junk-member shape, which emits no rows
        // for it either. The merged scanner+parser channel is the
        // established rig for this fixture class (`unclosed_formal_relocation`,
        // tests/gate_m0_wave3.rs `assert_codes_with_scanner`).
        let mut reported = false;
        while !self.cursor.at_eof() {
            if !matches!(
                self.peek_kind(),
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly
            ) {
                break;
            }
            if !reported {
                self.report(msg::expected_executable(), self.peek_span());
                reported = true;
            }
            self.cursor.advance();
        }
        if !reported && !self.cursor.at_eof() {
            // Follower continuation: the head `(` was skipped junk and the
            // boundary token is the junk-member face anchor.
            self.report(msg::unexpected_token(lparen_lexeme), lparen);
            self.report(msg::expected_executable(), self.peek_span());
        }
        false
    }

    /// Official `recoverySmallLookAheadSkipTokens` (`parser_impl.dart:2861`):
    /// bounded lookahead of up to 3 garbage tokens searching for a clause
    /// boundary in `lookFor`. A token that looks like the start of the next
    /// top-level declaration is a BAIL condition only — before the probe and
    /// after each step (:2870, :2882) it returns null WITHOUT reporting or
    /// consuming; success is a `lookFor` boundary alone (:2872-2874).
    /// Callers may sit ON a boundary (`{` with no clauses parsed) — there is
    /// nothing to skip and the boundary belongs to the caller. On success
    /// reports the skip (`unexpected_token` for a single skipped token,
    /// `unexpected_tokens` for a range) and leaves the cursor on the
    /// boundary. On failure the probe is non-destructive (official only
    /// walks `token.next` pointers and never consumes), so the cursor
    /// rewinds to the entry position and `false` is returned.
    fn skip_to_clause(&mut self, boundaries: &[TokenKind]) -> bool {
        let kind = self.peek_kind();
        if self.cursor.at_eof()
            || boundaries.contains(&kind)
            || Self::starts_next_top_level_declaration(kind)
        {
            return false;
        }
        let first = self.peek_span();
        let mark = self.cursor.mark();
        let mut last = first;
        let mut skipped = 0usize;
        while skipped < 3 {
            skipped += 1;
            let next = self.cursor.look_kind(1);
            if next == TokenKind::Eof {
                self.cursor.reset(mark);
                return false;
            }
            if boundaries.contains(&next) {
                if skipped == 1 {
                    self.report(msg::unexpected_token(self.cursor.lexeme(first.id)), first);
                } else {
                    self.report_with_end(msg::unexpected_tokens(), first, last);
                }
                self.cursor.advance();
                return true;
            }
            self.cursor.advance();
            last = self.peek_span();
            if Self::starts_next_top_level_declaration(self.peek_kind()) {
                self.cursor.reset(mark);
                return false;
            }
        }
        self.cursor.reset(mark);
        false
    }

    /// `looksLikeStartOfNextTopLevelDeclaration`
    /// (`identifier_context_impl.dart:1650`): top-level keywords plus the
    /// `const`/`get`/`final`/`set`/`var`/`void` member starters.
    fn starts_next_top_level_declaration(kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Keyword(
                Keyword::Class
                    | Keyword::Enum
                    | Keyword::Typedef
                    | Keyword::Mixin
                    | Keyword::Extension
                    | Keyword::Import
                    | Keyword::Export
                    | Keyword::Part
                    | Keyword::Library
                    | Keyword::Augment
                    | Keyword::Const
                    | Keyword::Get
                    | Keyword::Set
                    | Keyword::Final
                    | Keyword::Var
                    | Keyword::Void
            )
        )
    }
}

/// Modifier tokens captured by the shared declaration-modifier walk.
#[derive(Default)]
pub(crate) struct DeclModifiers {
    pub(crate) augment: Option<TokenSpan>,
    pub(crate) final_kw: Option<TokenSpan>,
}

/// Modifier tokens captured by the class-modifier walk.
#[derive(Default)]
struct ClassModifiers {
    abstract_kw: Option<TokenSpan>,
    augment: Option<TokenSpan>,
    final_kw: Option<TokenSpan>,
    late_kw: Option<TokenSpan>,
    covariant: Option<TokenSpan>,
}

fn is_builtin_identifier(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::BuiltIn)
}

fn msg_block_kind_class() -> crate::message::BlockKind {
    crate::message::BlockKind::Class
}
fn msg_block_kind_mixin() -> crate::message::BlockKind {
    crate::message::BlockKind::Mixin
}
fn msg_block_kind_enum() -> crate::message::BlockKind {
    crate::message::BlockKind::Enum
}
fn msg_block_kind_extension() -> crate::message::BlockKind {
    crate::message::BlockKind::Extension
}
fn msg_block_kind_extension_type() -> crate::message::BlockKind {
    crate::message::BlockKind::ExtensionType
}
