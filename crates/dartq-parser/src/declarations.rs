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
                if let Some(f) = mods.final_kw {
                    self.report(msg::final_enum(), f);
                }
                if let Some(b) = base {
                    self.report(msg::base_enum(), b);
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
                        // `extension<T> on ...` — the name is optional.
                        if let Some(end_group) = self.angle_group_after(2)
                            && self.kind_at_abs(end_group + 1) == TokenKind::Keyword(Keyword::On)
                        {
                            self.check_declaration();
                            let mods = self.parse_declaration_modifiers(modifier_start, keyword);
                            self.parse_extension(begin, mods.augment, keyword);
                            return;
                        }
                    } else if self.peek_kind() == TokenKind::Keyword(Keyword::Typedef) {
                        // `typedef <...> =` — missing the typedef name.
                        if let Some(end_group) = self.angle_group_after(2)
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
                            let class_keyword = self.peek_span_at(1);
                            let mods = self.parse_class_modifiers(modifier_start, class_keyword);
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
                            // `base` is consumed and reported by the dispatch lookahead.
                            let base_kw = None;
                            self.check_declaration();
                            self.parse_mixin(begin, mods.augment, base_kw, keyword);
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
                    self.report(msg::modifier_out_of_order("const", "class"), t);
                }
                TokenKind::Keyword(Keyword::External) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::modifier_out_of_order("external", "class"), t);
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
        if let Some(c) = mods.const_kw {
            let _ = c;
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
                    self.report(
                        msg::modifier_out_of_order("const", self.cursor.lexeme(keyword.id)),
                        t,
                    );
                }
                TokenKind::Keyword(Keyword::External) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(
                        msg::modifier_out_of_order("external", self.cursor.lexeme(keyword.id)),
                        t,
                    );
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
            let next_kind = self.kind_at(self.offset_of(skip) + 1);
            if next_kind == TokenKind::Eq || self.kind_at(self.offset_of(skip)) == TokenKind::Eq {
                new_style = true;
            } else if self.kind_at(self.offset_of(skip)) == TokenKind::Lt
                && let Some(end) = self
                    .cursor
                    .table_matching_end(self.real_id(self.offset_of(skip)))
                && self.kind_at(self.offset_of(end) + 1) == TokenKind::Eq
            {
                type_param = self.compute_type_param_or_arg_at(0, true);
                new_style = true;
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
            type_param.parse_variables(self, false);
            if self.peek_kind() == TokenKind::Eq {
                equals = Some(self.peek_span());
                self.cursor.advance();
                let type_info = self.compute_type(true, true);
                type_info.ensure_type_or_void(self);
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
            if self.peek_is_identifier() || self.peek_kind_ahead_is_paren_after_type() {
                let _ = self.ensure_identifier(IdentifierContext::TypedefDeclaration);
            } else {
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
        let name = self.ensure_identifier(IdentifierContext::EnumDeclaration);
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

    /// Official `parseEnumHeaderOpt` (with-clause, implements-clause, and
    /// out-of-order recovery).
    fn parse_enum_header_opt(&mut self, enum_keyword: TokenSpan) {
        let _ = enum_keyword;
        // `with` clause.
        let mut saw_with = false;
        if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
            self.parse_enum_with_clause();
            saw_with = true;
        } else {
            self.listener.handle_enum_no_with_clause();
        }
        // Skip garbage between clauses (official recoverySmallLookAheadSkip
        // reduced to a bounded skip of non-clause tokens).
        self.skip_to_clause(&[
            TokenKind::Keyword(Keyword::Implements),
            TokenKind::OpenCurly,
        ]);
        // implements clause.
        let mut saw_implements = false;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
            self.parse_implements_opt();
            saw_implements = true;
        } else {
            self.listener.handle_implements(None, 0);
        }
        // Out-of-order extras.
        while self.peek_kind() != TokenKind::OpenCurly && !self.cursor.at_eof() {
            if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
                self.report(
                    if saw_with {
                        msg::multiple_clauses("enum", "with")
                    } else {
                        msg::out_of_order_clauses("with", "implements")
                    },
                    self.peek_span(),
                );
                self.parse_enum_with_clause_suppressed();
                saw_with = true;
            } else if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
                self.report(
                    if saw_implements {
                        msg::multiple_implements()
                    } else {
                        msg::out_of_order_clauses("with", "implements")
                    },
                    self.peek_span(),
                );
                self.parse_implements_opt_suppressed();
            } else {
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
            self.parse_arguments_rest();
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

        if abstract_kw.is_some() {
            if let Some(s) = sealed {
                self.report(msg::abstract_sealed_class(), s);
            } else if let Some(f) = final_kw {
                if let Some(b) = base {
                    self.report_with_end(msg::abstract_final_base_class(), f, b);
                } else if let Some(i) = interface {
                    self.report_with_end(msg::abstract_final_interface_class(), f, i);
                }
            }
        }

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
            self.parse_single_type_required();
            while self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                self.parse_single_type_required();
            }
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
        self.parse_primary_constructor_opt(DeclarationKind::Class, const_keyword, true);
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
                // Recovery: out-of-order/duplicate clauses, then a block.
                self.parse_class_header_recovery(DeclarationHeaderKind::Class, header);
                self.ensure_block(msg_block_kind_class());
            }
            self.parse_class_or_mixin_or_extension_body(DeclarationKind::Class, name);
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
            // extends
            if matches!(self.peek_kind(), TokenKind::Keyword(_))
                && matches!(self.cursor.lexeme(self.peek_id()), "extend" | "on")
                && kind == DeclarationHeaderKind::Class
            {
                self.report(msg::expected_instead("extends"), self.peek_span());
                self.cursor.advance();
                let kw = self.peek_span();
                self.parse_class_extends_seen(kw, kind);
                clauses.extends = true;
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
                self.ensure_block(msg_block_kind_mixin());
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
            } else if matches!(self.peek_kind(), TokenKind::Keyword(_))
                && matches!(self.cursor.lexeme(self.peek_id()), "extend" | "extends")
            {
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
                self.ensure_block(msg_block_kind_extension());
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
                self.parse_class_header_recovery(
                    DeclarationHeaderKind::ExtensionType,
                    HeaderClauses::default(),
                );
                self.ensure_block(msg_block_kind_extension_type());
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

    /// Official `parsePrimaryConstructorOpt`.
    fn parse_primary_constructor_opt(
        &mut self,
        kind: DeclarationKind,
        const_keyword: Option<TokenSpan>,
        allow_representation: bool,
    ) {
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
                self.parse_formal_parameters_required_opt(
                    crate::listener::MemberKind::PrimaryConstructor,
                );
            } else {
                if matches!(kind, DeclarationKind::Class | DeclarationKind::Enum)
                    || (kind == DeclarationKind::ExtensionType && allow_representation)
                {
                    self.report(
                        msg::missing_primary_constructor_parameters(),
                        self.peek_span(),
                    );
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
            if kind == DeclarationKind::ExtensionType && allow_representation {
                self.report(msg::missing_primary_constructor(), self.peek_span());
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
    }

    /// Bounded clause-garbage skip (the `recoverySmallLookAheadSkipTokens`
    /// stand-in): skip tokens that cannot start a clause until a boundary.
    fn skip_to_clause(&mut self, boundaries: &[TokenKind]) {
        self.skip_to_sync(boundaries);
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
    const_kw: Option<TokenSpan>,
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
