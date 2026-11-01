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
use dartq_source::SourceRange;

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
use crate::types::is_valid_non_record_type_reference;
use crate::types::TypeInfo;

/// Whether a computed type is a named type — the `handleType` face an
/// `extends`/`implements`/`with`/`on` clause list entry must be
/// (`_popNamedTypeList`, ast_builder.dart:6313-6326).
///
/// `NoType` and `void` run the synthetic/named recovery (`expected_type_name`
/// / `invalid_void` carry their own rows) and count as named. `Complex` is
/// non-named exactly for the two shapes `_popNamedTypeList` rejects: a
/// top-level record type (`is_record_type`) and a generalized function type
/// (`gft_has_return_type`); a type-argumented or prefixed named type keeps a
/// named head and stays silent.
fn is_named_type(type_info: &TypeInfo) -> bool {
    match type_info {
        TypeInfo::NoType
        | TypeInfo::Void
        | TypeInfo::Simple
        | TypeInfo::SimpleNullable
        | TypeInfo::Prefixed
        | TypeInfo::SimpleWith1Arg { .. } => true,
        TypeInfo::Complex(c) => !c.is_record_type && c.gft_has_return_type.is_none(),
    }
}

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

impl<'s, L: ParserListener> Parser<'s, '_, '_, L> {
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
                            // `augment typedef <...> =` — the augment token
                            // rides into `parseTypedef` (official
                            // :774 wires `context.augmentToken`).
                            self.pending_augment = mods.augment;
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
                        // `augment typedef ...` — the augment token rides
                        // into `parseTypedef` (official :774/:792 wire
                        // `context.augmentToken`; `parseTypedef` reports
                        // `typedefAugmentation` on it).
                        self.pending_augment = mods.augment;
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
        // `compute_type` probes from the cursor, so walk it to the `(` head
        // first and restore — the probe itself is read-only; the mark/reset
        // only re-seats the cursor for the caller's dispatch.
        let mark = self.cursor.mark();
        self.cursor.advance();
        let type_info = self.compute_type(false, false);
        self.cursor.reset(mark);
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
    ///
    /// Duplicate modifiers never fall through into the class-name lane: the
    /// walk consumes every occurrence, records the first, and reports one
    /// `duplicated_modifier` row per later occurrence (upstream
    /// `_parseModifiers`, modifier_context.dart:351-389 — duplicate recovery
    /// in the per-keyword handlers `_parseAbstract` :406, `_parseAugment`
    /// :437, `_parseConst` :457, `_parseCovariant` :491, `_parseExternal`
    /// :529, `_parseFinal` :543, `_parseLate` :575, `_parseRequired` :598,
    /// `_parseStatic` :626, `_parseVar` :645). The const/external/
    /// extraneous faces anchor on the FIRST recorded token and fire after
    /// the walk, in the official `parseClassModifiers` order (:125-135:
    /// const, external, covariant, late, required, static, var).
    fn parse_class_modifiers(
        &mut self,
        modifier_start: crate::cursor::CursorMark,
        keyword: TokenSpan,
    ) -> ClassModifiers {
        self.cursor.reset(modifier_start);
        let mut mods = ClassModifiers::default();
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Abstract) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.abstract_kw.is_some() {
                        self.report(msg::duplicated_modifier("abstract"), t);
                    } else {
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
                }
                TokenKind::Keyword(Keyword::Augment) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.augment.is_some() {
                        self.report(msg::duplicated_modifier("augment"), t);
                    } else {
                        mods.augment = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Final) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.final_kw.is_some() {
                        self.report(msg::duplicated_modifier("final"), t);
                    } else {
                        if mods.late_kw.is_some() {
                            self.report(msg::modifier_out_of_order("final", "late"), t);
                        }
                        mods.final_kw = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Late) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.late_kw.is_some() {
                        self.report(msg::duplicated_modifier("late"), t);
                    } else {
                        mods.late_kw = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Const) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.const_kw.is_some() {
                        self.report(msg::duplicated_modifier("const"), t);
                    } else {
                        mods.const_kw = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::External) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.external.is_some() {
                        self.report(msg::duplicated_modifier("external"), t);
                    } else {
                        mods.external = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Covariant) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.covariant.is_some() {
                        self.report(msg::duplicated_modifier("covariant"), t);
                    } else {
                        mods.covariant = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Required) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.required.is_some() {
                        self.report(msg::duplicated_modifier("required"), t);
                    } else {
                        mods.required = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Static) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.static_kw.is_some() {
                        self.report(msg::duplicated_modifier("static"), t);
                    } else {
                        mods.static_kw = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Var) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.var_kw.is_some() {
                        self.report(msg::duplicated_modifier("var"), t);
                    } else {
                        mods.var_kw = Some(t);
                    }
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
        // Post-walk faces on the first recorded tokens (`parseClassModifiers`
        // modifier_context.dart:125-135; `reportTopLevelModifierError`
        // :676-700 dispatches const/external onto the `class` keyword).
        if let Some(t) = mods.const_kw {
            self.report(msg::const_class(), t);
        }
        if let Some(t) = mods.external {
            self.report(msg::external_class(), t);
        }
        if let Some(t) = mods.covariant {
            self.report(msg::extraneous_modifier("covariant"), t);
        }
        if let Some(t) = mods.late_kw {
            self.report(msg::extraneous_modifier("late"), t);
        }
        if let Some(t) = mods.required {
            self.report(msg::extraneous_modifier("required"), t);
        }
        if let Some(t) = mods.static_kw {
            self.report(msg::extraneous_modifier("static"), t);
        }
        if let Some(t) = mods.var_kw {
            self.report(msg::extraneous_modifier("var"), t);
        }
        let _ = keyword;
        mods
    }

    /// Shared modifier walk for enum/mixin/extension/typedef (`reportTopLevel
    /// ModifierError` shape: const/external are reported against the
    /// declaration keyword). Also the base walk of the directive modifier
    /// families (`directives::parse_directive_modifiers`).
    ///
    /// Duplicate modifiers never fall through into the name/declaration
    /// lane: every occurrence is consumed, the first is recorded, and each
    /// later one reports exactly one `duplicated_modifier` row (upstream
    /// `_parseModifiers`, modifier_context.dart:351-389 over
    /// `parseEnumModifiers` :140, `parseMixinModifiers` :168,
    /// `parseExtensionModifiers` :154, `parseTopLevelKeywordModifiers` :183
    /// and `parseTypedefModifiers` :314 — duplicate recovery in the
    /// per-keyword handlers `_parseAbstract` :406, `_parseAugment` :437,
    /// `_parseConst` :457, `_parseCovariant` :491, `_parseExternal` :529,
    /// `_parseFinal` :543, `_parseLate` :575, `_parseRequired` :598,
    /// `_parseStatic` :626, `_parseVar` :645). The recorded first tokens
    /// keep the family rows (`final enum`, `final mixin`, the directive
    /// `final`/`augment` extraneous rows) anchored on the first occurrence.
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
                    if mods.const_kw.is_some() {
                        self.report(msg::duplicated_modifier("const"), t);
                    } else {
                        mods.const_kw = Some(t);
                        self.report(msg::extraneous_modifier("const"), t);
                    }
                }
                TokenKind::Keyword(Keyword::External) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.external.is_some() {
                        self.report(msg::duplicated_modifier("external"), t);
                    } else {
                        mods.external = Some(t);
                        match self.cursor.lexeme(keyword.id) {
                            "enum" => self.report(msg::external_enum(), t),
                            "typedef" => self.report(msg::external_typedef(), t),
                            _ => self.report(msg::extraneous_modifier("external"), t),
                        }
                    }
                }
                TokenKind::Keyword(Keyword::Abstract) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.abstract_kw.is_some() {
                        self.report(msg::duplicated_modifier("abstract"), t);
                    } else {
                        mods.abstract_kw = Some(t);
                        self.report(msg::extraneous_modifier("abstract"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Covariant) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.covariant.is_some() {
                        self.report(msg::duplicated_modifier("covariant"), t);
                    } else {
                        mods.covariant = Some(t);
                        self.report(msg::extraneous_modifier("covariant"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Late) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.late_kw.is_some() {
                        self.report(msg::duplicated_modifier("late"), t);
                    } else {
                        mods.late_kw = Some(t);
                        self.report(msg::extraneous_modifier("late"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Required) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.required.is_some() {
                        self.report(msg::duplicated_modifier("required"), t);
                    } else {
                        mods.required = Some(t);
                        self.report(msg::extraneous_modifier("required"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Static) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.static_kw.is_some() {
                        self.report(msg::duplicated_modifier("static"), t);
                    } else {
                        mods.static_kw = Some(t);
                        self.report(msg::extraneous_modifier("static"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Var) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.var_kw.is_some() {
                        self.report(msg::duplicated_modifier("var"), t);
                    } else {
                        mods.var_kw = Some(t);
                        self.report(msg::extraneous_modifier("var"), t);
                    }
                }
                TokenKind::Keyword(Keyword::Final) => {
                    // Captured: the declaration family decides whether `final`
                    // is an error (`final enum`, `final mixin`) or silently
                    // accepted (extension, official parseExtensionModifiers).
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.final_kw.is_some() {
                        self.report(msg::duplicated_modifier("final"), t);
                    } else {
                        mods.final_kw = Some(t);
                    }
                }
                TokenKind::Keyword(Keyword::Augment) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    if mods.augment.is_some() {
                        self.report(msg::duplicated_modifier("augment"), t);
                    } else {
                        mods.augment = Some(t);
                    }
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

    /// Degraded function-type alias probe: `Some(offset)` of the `Function`
    /// intro when the alias RHS is a generalized function type whose
    /// type-variable group never validates, so the official classification
    /// falls back to the bare return type (`computeVoidGFT` /
    /// `computeIdentifierGFT`, type_info_impl.dart:949/753). The group's
    /// name position classifies as noType in `ComplexTypeParamOrArgInfo`
    /// `.compute` (reserved heads and the member-keyword built-ins the type
    /// recognizer rejects: `get`/`set`/`factory`/`operator`/`typedef` name)
    /// with a bound or junk run after the name — the recovery skip then
    /// sits in front of the name and `computeRest`'s `(`-check fails.
    /// Identifiers, `required`-class built-ins, `dynamic`/`Function`/`void`
    /// and immediate closers all validate, like the official compute loop.
    fn degraded_gft_return_type_end(&self) -> Option<usize> {
        // The return type cannot contain `Function`; the first one is the
        // function-type intro.
        let mut intro = self.cursor.real_pos();
        loop {
            match self.kind_at(intro) {
                TokenKind::Keyword(Keyword::Function) => break,
                TokenKind::Semicolon | TokenKind::Eof => return None,
                _ => intro += 1,
            }
        }
        if self.kind_at(intro + 1) != TokenKind::Lt {
            return None;
        }
        let name = self.kind_at(intro + 2);
        if matches!(name, TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt) {
            // Empty group (`<>`): no name position, the classification
            // validates.
            return None;
        }
        let follower = self.kind_at(intro + 3);
        let name_is_type = is_valid_non_record_type_reference(name, follower)
            || name == TokenKind::OpenParen
            || matches!(
                name,
                TokenKind::Keyword(kw)
                    if kw.style() == dartq_scanner::KeywordStyle::BuiltIn
                        && !matches!(
                            kw,
                            Keyword::Get
                                | Keyword::Set
                                | Keyword::Factory
                                | Keyword::Operator
                        )
                        && !(kw == Keyword::Typedef && follower == TokenKind::Identifier),
            );
        let follower_is_closer = matches!(
            follower,
            TokenKind::Gt | TokenKind::GtGt | TokenKind::GtGtGt | TokenKind::Comma
        );
        (!name_is_type && !follower_is_closer).then_some(intro)
    }

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
        // Official `newStyleParseAsRecovered` (parser_impl.dart:1466): `true`
        // only for the `name <typeVars>? =` shape — the `=` after the group
        // forces the head into the name slot under the recovered face. The
        // `skip is EQ` (missing-name `typedef <T> =`) and `<`-recheck arms
        // keep `false` (official :1491/:1513-1518).
        let mut name_recovered = false;
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
            if next_kind == TokenKind::Eq {
                new_style = true;
                name_recovered = true;
            } else if self.kind_at(self.offset_of(skip)) == TokenKind::Eq {
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
            // The keyword-as-typedef-name built-in face (see
            // `builtin_typedef_name_row`): captured ahead of the name
            // recovery, emitted after it — the parser's keyword-name row
            // precedes the verifier-derived built-in row in the oracle
            // output.
            let builtin_row = self.builtin_typedef_name_row();
            let name = if name_recovered {
                self.ensure_identifier_recovered(IdentifierContext::TypedefDeclaration)
            } else {
                self.ensure_identifier(IdentifierContext::TypedefDeclaration)
            };
            if let Some((at, kind)) = builtin_row {
                self.report_builtin_typedef_name_at(at, kind);
            }
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
                // `nonfunction-type-aliases` (`enabledIn: 2.13`): a new-style
                // alias whose right-hand type is not a function type reports
                // `experiment_not_enabled` at the `=` below the boundary
                // (`endGenericTypeAlias`, ast_builder.dart:3373).
                if !type_is_gft
                    && !self.features.nonfunction_type_aliases_enabled
                    && let Some(eq) = equals
                {
                    self.report(msg::experiment_not_enabled("nonfunction-type-aliases"), eq);
                }
                let mut handled = false;
                if matches!(type_info, crate::types::TypeInfo::NoType)
                    && self.peek_kind() == TokenKind::Lt
                    && let Some(end_group) = self.angle_group_after(0)
                {
                    let closer = self.span_of(self.real_id(end_group));
                    match self.kind_at(end_group + 1) {
                        TokenKind::Semicolon => {
                            // Official :1560-1568: `typedef i = <...>;` — the
                            // type arguments belong to a missing `Function`.
                            // The synthetic `Function` keyword is inserted
                            // ahead of the group (`insertSyntheticKeyword`,
                            // :1561) and reports `expectedButGot('Function')`
                            // — re-anchored onto the `<` by the zero-width
                            // walk (oracle `typedef i = <int>;`: the row sits
                            // on the `<`, len 1) — then the missing parameter
                            // list is reported on the group's closer (:1551)
                            // and the alias parses as `Function<...>();`.
                            let lt = self.peek_span();
                            self.report(msg::expected_token("Function"), lt);
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
                    // `Function`. The type itself still parses as a type
                    // (the oracle reports its head and each unresolved type
                    // argument beside the missing-`Function` row), so the
                    // events drive through the normal type parse; the
                    // read-only `skip_type` only probes the boundary.
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
                        type_info.ensure_type_or_void(self);
                        self.parse_formal_parameters_required_opt(
                            crate::listener::MemberKind::GeneralizedFunctionType,
                        );
                        handled = true;
                    }
                }
                if let Some(intro) = self.degraded_gft_return_type_end() {
                    // Degraded alias face (see the probe): the alias type is
                    // the bare return type. Consume it and close on it —
                    // `ensureSemicolon` anchors `expectedToken(';')` on the
                    // return type's last token (parser_impl.dart:5048) — and
                    // let the declaration loop re-read the `Function<...>(...)`
                    // tail as a top-level member whose `;` reports
                    // `missingFunctionBody` (`parseFunctionBody`,
                    // `diag.expectedBody`). The recovery arms above cannot
                    // fire here: the return type's follower is `Function`,
                    // never `(` or `<`.
                    self.skip_to_token(self.real_id(intro));
                } else if !handled {
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
                // at the name position. The official recovery reports
                // `expectedToken(';')` anchored on the type head
                // (ensureSemicolon's walked-back anchor), then the typedef
                // parse itself finishes against the `=`: the name position
                // reports `missing_identifier` on the `=`
                // (insertSyntheticIdentifier's consume-in-doubt branch) and
                // the required parameter list reports
                // `missing_typedef_parameters` on the same token before the
                // synthetic `()` splices in (`parseFormalParametersRequiredOpt`,
                // parser_impl.dart:1913-1918); the `=` stays for the
                // declaration loop, whose member reparse mints the
                // expected_executable face.
                let head_end = self
                    .cursor
                    .last_consumed_id()
                    .map_or_else(|| self.peek_span(), |id| self.span_of(id));
                self.report(msg::expected_token(";"), head_end);
                let eq = self.peek_span();
                self.report(msg::missing_identifier(), eq);
                self.report(msg::missing_typedef_parameters(), eq);
                self.listener
                    .end_typedef(augment, typedef_keyword, None, head_end);
                return;
            }
            // The keyword-as-typedef-name built-in face: any keyword head
            // (the verifier's isKeyword-and-not-pseudo gate — a reserved
            // word carries the row exactly like a built-in one, oracle
            // keyword_named_typedefs.dart 5|14 `typedef void assert();`).
            // It must not sit behind the identifier-shape guard a reserved
            // head fails (`isIdentifier` rejects reserved words). Captured
            // ahead of the name recovery, emitted after it — the parser's
            // keyword-name row precedes the built-in row in the oracle
            // output. After the `=` early-return: `typedef void = ...;`
            // (the missing-name shape) never reaches here, matching the
            // oracle's silent synthetic-name face on
            // keyword_named_typedefs.dart 132.
            let builtin_row = self.builtin_typedef_name_row();
            // Official `isIdentifierRecovered` (parser_impl.dart:1590-1594):
            // a non-identifier head is treated as the name when the token
            // after the type-parameter group is `(` (`typedef void assert();`).
            // Anything else takes the context's normal `ensureIdentifier`
            // template — a reserved word heading a top-level declaration
            // splices a synthetic name and stays for the declaration loop
            // (oracle `typedef var a;`: `missing_identifier` at `var`,
            // `expected_token ';'` anchored on `typedef`).
            // Official `isIdentifierRecovered` (parser_impl.dart:1593):
            // `next.kind != IDENTIFIER_TOKEN` is the RAW token kind — a
            // built-in or pseudo keyword (`get`/`set`/`mixin`) is NOT an
            // `IDENTIFIER_TOKEN`, so it is recovered exactly like a reserved
            // word. `peek_is_identifier` (which admits built-ins) would route
            // `typedef void get();` to the plain `ensureIdentifier` template
            // and lose the keyword-name face; the raw-kind test keeps the
            // `(`/`=` shapes on the recovered variant.
            let is_recovered = self.peek_kind() != TokenKind::Identifier
                && self.kind_at(self.offset_of(type_param.skip(self.peek_id())) + 1)
                    == TokenKind::OpenParen;
            if is_recovered {
                let _ = self.ensure_identifier_recovered(IdentifierContext::TypedefDeclaration);
            } else {
                let _ = self.ensure_identifier(IdentifierContext::TypedefDeclaration);
            }
            if let Some((at, kind)) = builtin_row {
                self.report_builtin_typedef_name_at(at, kind);
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

    /// Pre-capture of the keyword-as-typedef-name built-in face (oracle
    /// `keyword_named_typedefs.dart`: one `built_in_identifier_in_declaration`
    /// row per KEYWORD taking the name position, both typedef styles; the
    /// oracle face is the verifier's `_checkForBuiltInIdentifierAsName`
    /// (error_verifier.dart:3760-3766: `token.type.isKeyword &&
    /// token.keyword?.isPseudo != true`), so RESERVED words carry it exactly
    /// like built-in ones (`typedef void assert();` rows at 5|14 and 6|9)
    /// and pseudo keywords never do. The analyzer converter drops the shared
    /// parser's `builtInIdentifierInDeclaration` message in favor of that
    /// verifier face (error_converter.dart:291-295), whose per-context
    /// template names the declaration kind — the parser fires that face where
    /// the peek token is about to become the name. The follower test reads
    /// the stream before the name parse consumes the head, so the capture
    /// must happen there; [`Self::report_builtin_typedef_name_at`] emits
    /// after the name parse, keeping the parser row (keyword-name face)
    /// ahead of the built-in row.
    fn builtin_typedef_name_row(&self) -> Option<(TokenSpan, TokenKind)> {
        if let TokenKind::Keyword(kw) = self.peek_kind()
            // The verifier gate (error_verifier.dart:3767): any non-pseudo
            // keyword — built-in and reserved alike — plus, under the
            // variance experiment, its pseudo `inout`/`out` branch
            // (:3774-3780).
            && (kw.style() != dartq_scanner::KeywordStyle::Pseudo
                || (self.features.variance_enabled
                    && matches!(kw, Keyword::Inout | Keyword::Out)))
            && matches!(
                self.cursor.look_kind(1),
                TokenKind::OpenParen
                    | TokenKind::Lt
                    | TokenKind::Eq
                    | TokenKind::Semicolon
                    | TokenKind::Eof
            )
        {
            return Some((self.peek_span(), self.peek_kind()));
        }
        None
    }

    fn report_builtin_typedef_name_at(&mut self, at: TokenSpan, kind: TokenKind) {
        if let TokenKind::Keyword(_) = kind {
            let lexeme = self.cursor.lexeme(at.id);
            self.report(
                msg::built_in_identifier_as_name(msg::BuiltInNameKind::Typedef, lexeme),
                at,
            );
        }
    }

    // ------------------------------------------------------------------
    // declaration name
    // ------------------------------------------------------------------

    /// Annotation junk in a declaration-name position, official
    /// `ClassOrMixinOrExtensionIdentifierContext.ensureIdentifier` /
    /// `EnumDeclarationIdentifierContext.ensureIdentifier` branch 3
    /// (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/
    /// identifier_context_impl.dart): `@meta` is not keyword-shaped and
    /// sits outside the templates' insert-before keep-sets, so it is
    /// reported and consumed in doubt —
    /// `rewriter.insertSyntheticIdentifier(identifier)` splices the
    /// synthetic name AFTER the junk, at the follower's offset, leaving
    /// the junk behind the parse pointer. The generic
    /// [`Parser::ensure_identifier`] table instead parks its synthetic at
    /// the junk's own offset (the `@` reads as a statement start there),
    /// which re-anchors every downstream header face on the `@` — the
    /// official zero-width name carries the reports FORWARD onto the junk
    /// identifier that follows it (`findNonZeroLengthToken`, util.dart:56;
    /// corpus Primary-constructors grammar_A06_t01: the oracle anchors the
    /// class/mixin/enum/extension-type header rows on `meta`, never the
    /// `@`).
    fn ensure_declaration_name(&mut self, context: IdentifierContext) -> TokenSpan {
        if self.peek_kind() == TokenKind::At {
            let at = self.peek_span();
            self.report(msg::missing_identifier(), at);
            self.cursor.advance(); // the junk — consumed in doubt
            let follower = self.peek_span();
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, follower.range.offset);
            self.cursor.advance(); // the synthetic name
            return self.span_of(id);
        }
        self.ensure_identifier(context)
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
        // Official `EnumDeclarationIdentifierContext.ensureIdentifier`
        // (identifier_context_impl.dart:298-337): a reserved word that
        // starts the next top-level declaration (arm 2,
        // `looksLikeStartOfNextTopLevelDeclaration`) splices a synthetic
        // name and leaves the keyword for the declaration loop; any other
        // reserved word takes `expectedIdentifierButGotKeyword` and IS the
        // name (arm 5 — oracle `enum if {}`); built-in keywords are consumed
        // as the name over the converter-suppressed parser face (arm 3);
        // pseudo keywords pass through (arm 1). Non-keyword junk keeps the
        // generic `ensure_declaration_name` path (arm 4).
        let reserved_head = matches!(
            self.peek_kind(),
            TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Reserved
        );
        let built_in_name = if matches!(
            self.peek_kind(),
            TokenKind::Keyword(kw) if kw.style() != dartq_scanner::KeywordStyle::Pseudo
                || (self.features.variance_enabled
                    && matches!(kw, Keyword::Inout | Keyword::Out))
        ) && !(reserved_head && Self::starts_next_top_level_declaration(self.peek_kind()))
        {
            // The verifier's built-in-name face (error_verifier.dart:846-849,
            // `_checkForBuiltInIdentifierAsName(node.namePart.typeName,
            // builtInIdentifierAsTypeName)`; the gate at :3372 is
            // `isKeyword && isPseudo != true` — reserved words carry the row
            // exactly like built-in ones). Captured ahead of the name parse,
            // emitted after the type-parameter parse (the class-lane
            // pattern): the name must already be consumed so the capture
            // reads the token about to take the name position.
            Some(self.peek_span())
        } else {
            None
        };
        let name = if reserved_head && Self::starts_next_top_level_declaration(self.peek_kind()) {
            let at = self.peek_span();
            self.report(msg::missing_identifier(), at);
            self.synthetic(TokenKind::Identifier, at.range.offset)
        } else if reserved_head {
            let at = self.peek_span();
            let lexeme = self.cursor.lexeme(at.id).to_owned();
            self.report(msg::expected_identifier_but_got_keyword(&lexeme), at);
            self.cursor.advance();
            at
        } else {
            self.ensure_declaration_name(IdentifierContext::EnumDeclaration)
        };
        // Official parseEnum passes inDeclaration: true, allowsVariance: true
        // (parser_impl.dart:2629): the header accepts variance modifiers.
        let type_param = self.compute_type_param_or_arg_on_lt(true);
        let type_param_lt = if matches!(type_param, crate::types::TypeParamOrArg::None) {
            None
        } else {
            Some(self.peek_span())
        };
        type_param.parse_variables(self, true);
        if let Some(at) = built_in_name {
            let lexeme = self.cursor.lexeme(at.id).to_owned();
            self.report(
                msg::built_in_identifier_as_name(msg::BuiltInNameKind::Type, &lexeme),
                at,
            );
        }
        self.listener
            .begin_enum_declaration(begin, augment, enum_keyword, name);
        self.parse_primary_constructor_opt(DeclarationKind::Enum, Some(name), const_keyword, true);
        // The `enhanced-enums` header face (`handleEnumHeader`,
        // ast_builder.dart:4131): below 2.17 one report, anchored on the
        // first present construct — the with-clause, else the
        // implements-clause, else the type parameter list's `<`.
        let header_anchor = self.parse_enum_header_opt(enum_keyword);
        if let Some(at) = header_anchor.or(type_param_lt) {
            self.report_enhanced_enums_gate(at);
        }

        let mut left_brace = self.peek_span();
        let mut member_count = 0usize;
        if self.peek_kind() == TokenKind::Semicolon {
            let semi = self.peek_span();
            self.cursor.advance();
            // The enum empty-body face (`parseEnumDeclaration`,
            // parser_impl.dart:2623-2631): an `enum E;` body below 3.13
            // reports `experiment_not_enabled` at the `;`.
            if !self.features.primary_constructors_enabled {
                self.report_experiment_not_enabled(semi);
            }
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
                // The `enhanced-enums` elements-end face (`handleEnumElements`,
                // ast_builder.dart:4094): a `;` closing the constant list is
                // the enhanced form below 2.17.
                self.report_enhanced_enums_gate(self.peek_span());
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
                        self.report(msg::expected_executable(), self.peek_span());
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
    /// orders them. Returns the anchor of the first successfully parsed
    /// clause (the `with` keyword, else the `implements` keyword) — the
    /// `enhanced-enums` gate's report site.
    fn parse_enum_header_opt(&mut self, _enum_keyword: TokenSpan) -> Option<TokenSpan> {
        const LOOK: [TokenKind; 3] = [
            TokenKind::OpenCurly,
            TokenKind::Keyword(Keyword::With),
            TokenKind::Keyword(Keyword::Implements),
        ];
        let mut header_anchor: Option<TokenSpan> = None;
        // Gunk before any clause (:2744-2752).
        if !LOOK.contains(&self.peek_kind()) {
            self.skip_to_clause(&LOOK);
        }
        // With-clause opt (:2754).
        let saw_with = if self.peek_kind() == TokenKind::Keyword(Keyword::With) {
            header_anchor = Some(self.peek_span());
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
            header_anchor.get_or_insert(self.peek_span());
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
        header_anchor
    }

    /// The `enhanced-enums` gate (`enabledIn: 2.17`): the analyzer reports
    /// `experiment_not_enabled` from the enum element/header handlers
    /// (ast_builder.dart:4035/:4094/:4131) when the feature is off.
    fn report_enhanced_enums_gate(&mut self, at: TokenSpan) {
        if !self.features.enhanced_enums_enabled {
            self.report(msg::experiment_not_enabled("enhanced-enums"), at);
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
        self.parse_single_type_required(msg::NamedTypeClause::Implements);
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.parse_single_type_required(msg::NamedTypeClause::Implements);
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
        // Official `parser_impl.dart` `parseEnumElement`: an `augment`
        // keyword directly before the constant's identifier is the
        // constant's own augmentation mark — consumed inline (not through
        // `pending_augment`, which only top-level declaration dispatch
        // sets).
        let augment = if self.peek_kind() == TokenKind::Keyword(Keyword::Augment) {
            let at = self.peek_span();
            self.cursor.advance();
            Some(at)
        } else {
            self.pending_augment.take()
        };

        self.ensure_identifier(IdentifierContext::EnumValueDeclaration);
        let constructor_start = self.peek_span();
        let mut has_type_arguments_or_dot = false;
        {
            let start = constructor_start;
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
        let has_arguments = self.peek_kind() == TokenKind::OpenParen;
        if has_arguments || has_type_arguments_or_dot {
            // The `enhanced-enums` element face (`handleEnumElement`,
            // ast_builder.dart:4035-4050): a constant with arguments (at the
            // `(`) or a named/generic constructor selector (at the
            // constructor reference head) is the enhanced form below 2.17.
            let anchor = if has_arguments {
                self.peek_span()
            } else {
                constructor_start
            };
            self.report_enhanced_enums_gate(anchor);
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
        // The analyzer's off-version modifier faces (`beginClassDeclaration`
        // ast_builder.dart:241-283, `beginNamedMixinApplication` :502-554 —
        // one `experiment_not_enabled` row per present modifier, anchored on
        // the modifier token, which is then dropped from the declaration):
        // `sealed` under the sealed-class gate, `base`/`interface`/`final`
        // and `mixin class`'s `mixin` under the class-modifiers gate.
        if let Some(t) = sealed
            && !self.features.sealed_class_enabled
        {
            self.report(msg::experiment_not_enabled("sealed-class"), t);
        }
        if !self.features.class_modifiers_enabled {
            if let Some(t) = base {
                self.report(msg::experiment_not_enabled("class-modifiers"), t);
            }
            if let Some(t) = interface {
                self.report(msg::experiment_not_enabled("class-modifiers"), t);
            }
            if let Some(t) = final_kw {
                self.report(msg::experiment_not_enabled("class-modifiers"), t);
            }
            if let Some(t) = mixin {
                self.report(msg::experiment_not_enabled("class-modifiers"), t);
            }
        }
        self.listener
            .begin_class_or_mixin_or_named_mixin_application_prelude(begin);
        self.cursor.advance(); // `class`

        let mut const_keyword: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Const) {
            const_keyword = Some(self.peek_span());
            self.cursor.advance();
        }
        // The built-in-name face is captured here but emitted after the
        // type-parameter parse: the verifier visit decides the template by
        // the declaration shape — a named mixin application (`class C = A
        // with B;`) reports the typedef-name text (visitClassTypeAlias,
        // error_verifier.dart:635), a plain class the type-name text
        // (:573) — and the `=` only surfaces once the `<...>` group is
        // consumed. ensure_identifier consumes the keyword silently (the
        // keyword-name gate carries no arm for this context).
        let built_in_name = if self.is_built_in_or_reserved_name_token(0) {
            Some(self.peek_span())
        } else {
            None
        };
        let name =
            self.ensure_declaration_name(IdentifierContext::ClassOrMixinOrExtensionDeclaration);
        // Official parseClassOrNamedMixinApplication passes inDeclaration:
        // true, allowsVariance: true (parser_impl.dart:2981): the header
        // accepts variance modifiers (covers named mixin applications too —
        // same entry).
        let type_param = self.compute_type_param_or_arg_on_lt(true);
        type_param.parse_variables(self, true);
        // Emit the built-in-name row only when the keyword was actually
        // consumed as the declaration name — the captured token id survives
        // in `name.id`. When the recovery arm splices a synthetic name at the
        // keyword's offset (`class enum E { v }`: `enum` stays for the enum
        // reparse) `name.id` is the synthetic identifier, not the keyword —
        // the keyword never became the class name, so the verifier's
        // `_checkForBuiltInIdentifierAsName` has no name to report.
        if let Some(at) = built_in_name
            && at.id == name.id
        {
            let kind = if self.peek_kind() == TokenKind::Eq {
                msg::BuiltInNameKind::Typedef
            } else {
                msg::BuiltInNameKind::Type
            };
            self.report(
                msg::built_in_identifier_as_name(kind, self.cursor.lexeme(at.id)),
                at,
            );
        }

        // Official modifier-trio check (parser_impl.dart:2975-3001): an
        // `abstract` class paired with `sealed`, or `final` paired with
        // `base`/`interface`, reports here — before the mixin-application
        // split — anchored on the pair's first keyword.
        if abstract_kw.is_some() {
            if let Some(sealed_tok) = sealed {
                self.report(msg::abstract_sealed_class(), sealed_tok);
            } else if let Some(final_tok) = final_kw {
                if let Some(base_tok) = base {
                    self.report_with_end(msg::abstract_final_base_class(), final_tok, base_tok);
                } else if let Some(interface_tok) = interface {
                    self.report_with_end(
                        msg::abstract_final_interface_class(),
                        final_tok,
                        interface_tok,
                    );
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
    ///
    /// The implements clause emits the real `handleImplements` event (the
    /// official diet parser folds it into `endNamedMixinApplication`; the
    /// dartq event stream carries clause names through `handle_implements`
    /// so the semantic collector attributes them to the implements face
    /// instead of the header residue).
    fn parse_named_mixin_application(&mut self, begin: TokenSpan, class_keyword: TokenSpan) {
        let equals = self.peek_span();
        self.cursor.advance();
        let type_begin = self.peek_span();
        let type_info = self.compute_type(true, false);
        let type_end = type_info.ensure_type_not_void(self);
        self.check_named_type(
            &type_info,
            type_begin,
            type_end,
            msg::NamedTypeClause::Extends,
        );
        self.parse_mixin_application_rest();
        let mut implements = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
            implements = Some(self.peek_span());
            self.cursor.advance();
            let mut count = 1usize;
            self.parse_single_type_required(msg::NamedTypeClause::Implements);
            while self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                self.parse_single_type_required(msg::NamedTypeClause::Implements);
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

    /// Official `parseTypeList`: `type (',' type)*`. Used only for the
    /// `with` clause family (class / enum / named-mixin / mixin recovery).
    fn parse_type_list(&mut self) {
        let begin = self.peek_span();
        self.listener.begin_type_list(begin);
        let mut count = 1usize;
        self.parse_single_type_required(msg::NamedTypeClause::With);
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.parse_single_type_required(msg::NamedTypeClause::With);
            count += 1;
        }
        self.listener.end_type_list(count);
    }

    /// One required clause-list type
    /// (`computeType(required: true).ensureTypeNotVoid`, plus the
    /// `_popNamedTypeList` named-type check).
    fn parse_single_type_required(&mut self, clause: msg::NamedTypeClause) {
        let begin = self.peek_span();
        let type_info = self.compute_type(true, false);
        let end = type_info.ensure_type_not_void(self);
        self.check_named_type(&type_info, begin, end, clause);
    }

    /// The `_popNamedTypeList` check (`ast_builder.dart:6313-6326`): a clause
    /// type that is not a named type reports `expected_named_type` over its
    /// own span (official `locatableDiagnostic.at(type)`); recovery continues
    /// with the type already consumed (upstream drops the entry from the
    /// built list, a parse-tier no-op).
    fn check_named_type(
        &mut self,
        type_info: &TypeInfo,
        begin: TokenSpan,
        end: TokenSpan,
        clause: msg::NamedTypeClause,
    ) {
        if !is_named_type(type_info) {
            self.report(
                msg::expected_named_type(clause),
                self.span_through(begin, end),
            );
        }
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

    /// The verifier's built-in-name gate extended to reserved words: the
    /// report fires on every keyword that is not a pseudo keyword
    /// (`error_verifier.dart:846-849`, gate `isKeyword && isPseudo != true`
    /// at :3372). Oracle probes: `class if {}`, `mixin if {}`, and
    /// `extension type if(int i) {}` each carry
    /// `BUILT_IN_IDENTIFIER_IN_DECLARATION` on the keyword beside the
    /// parser's `expected_identifier_but_got_keyword`; `extension if on int
    /// {}` does not — its header junk recovery never makes the keyword the
    /// name, so the verifier has no name token to fire on, and the
    /// extension lane keeps its identifier-shaped gate.
    fn is_built_in_or_reserved_name_token(&self, n: usize) -> bool {
        self.is_built_in_name_token(n)
            || matches!(
                self.cursor.look_kind(n),
                TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Reserved
            )
    }

    /// Official `parseClass`; the cursor sits after the type variables.
    fn parse_class(
        &mut self,
        begin: TokenSpan,
        class_keyword: TokenSpan,
        const_keyword: Option<TokenSpan>,
        name: TokenSpan,
    ) {
        // `headerStart` (parser_impl.dart:3098): the position before the
        // primary-constructor parameters — the missing-body recovery
        // restarts from here, so an unconsumed-looking `(` ahead of a `{`
        // takes the skipped-junk face (`class C({var int a})` at EOF).
        let header_start = self.cursor.mark();
        if self.parse_primary_constructor_opt(DeclarationKind::Class, Some(name), const_keyword, true)
        {
            // The "params consumed" fact must be captured here — before the
            // clause walk advances the cursor. Measured at recovery time the
            // clause tokens (`extends A`, …) have already moved the cursor
            // past `headerStart`, so every class looked param-consuming and
            // the rewind re-walked the first pass's clauses, double-firing
            // their faces (the official's learning reparse swallows those
            // events; ours report directly).
            let params_consumed =
                self.cursor.real_pos() > crate::cursor::TokenCursor::mark_pos(header_start);
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
                    // Recovery for out-of-order/duplicate clauses. Rewind to
                    // `headerStart` ONLY when the primary-constructor header
                    // consumed tokens (the oracle anchors `expected_body` on
                    // the `(` and re-forms the body from the params — probed
                    // 3.13.1, `class C(int a)`); the recovery loop's junk-skip
                    // refuses a `(` whose follower is not a clause keyword, so
                    // the rewound walk jams there and nothing re-reports.
                    // Without primary-constructor tokens the first pass's
                    // clauses already reported their faces — rewinding would
                    // re-walk them and double-fire, so the recovery continues
                    // from the first pass's end (`paramsConsumed` was captured
                    // right after `parse_primary_constructor_opt`, before the
                    // clause walk moved the cursor).
                    if params_consumed {
                        self.cursor.reset(header_start);
                    }
                    self.parse_class_header_recovery(DeclarationHeaderKind::Class, header);
                }
                self.parse_class_or_mixin_or_extension_body(DeclarationKind::Class, name);
            }
        } else {
            // The class-head `(` junk walk consumed the tail; the
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
            self.cursor.drain_to(close.id);
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
        // native clause (deprecated). The official report lives in the
        // resolver (`visitNativeClause`, error_verifier.dart:1324-1329,
        // `!_isInSystemLibrary`) with the clause-node span — keyword through
        // the string when named (probed 3.13.1: `native "corpus"` len 15,
        // bare `native` len 6); dartq reports at the parse tier, where the
        // non-SDK gate is trivially true.
        let mut native = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Native) {
            let native_kw = self.peek_span();
            self.cursor.advance();
            let has_name = self.peek_kind() == TokenKind::String;
            let mut end = native_kw;
            if has_name {
                end = self.parse_literal_string();
            }
            self.listener.handle_native_clause(native_kw, has_name);
            self.report_with_end(msg::native_clause_in_non_sdk_code(), native_kw, end);
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
        self.parse_single_type_required(msg::NamedTypeClause::Implements);
        while self.peek_kind() == TokenKind::Comma {
            self.cursor.advance();
            self.parse_single_type_required(msg::NamedTypeClause::Implements);
            count += 1;
        }
        self.listener.handle_implements(Some(implements), count);
        true
    }

    /// Official `parseClassExtendsSeenExtendsClause`.
    fn parse_class_extends_seen(&mut self, extends_kw: TokenSpan, kind: DeclarationHeaderKind) {
        let type_begin = self.peek_span();
        let type_info = self.compute_type(true, false);
        let type_end = type_info.ensure_type_not_void(self);
        self.check_named_type(
            &type_info,
            type_begin,
            type_end,
            msg::NamedTypeClause::Extends,
        );
        let mut count = 1usize;
        if self.peek_kind() == TokenKind::Comma {
            if kind == DeclarationHeaderKind::Class {
                self.report(msg::multiple_extends(), self.peek_span());
            }
            while self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                let type_begin = self.peek_span();
                let type_info = self.compute_type(true, false);
                let type_end = type_info.ensure_type_not_void(self);
                self.check_named_type(
                    &type_info,
                    type_begin,
                    type_end,
                    msg::NamedTypeClause::Extends,
                );
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
                self.parse_single_type_required(msg::NamedTypeClause::Implements);
                while self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    self.parse_single_type_required(msg::NamedTypeClause::Implements);
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
        // `beginMixinDeclaration`'s off-version face (ast_builder.dart:
        // 487-494): `base mixin M` below 3.0 reports `experiment_not_enabled`
        // at the `base` token, which is then dropped.
        if let Some(t) = base
            && !self.features.class_modifiers_enabled
        {
            self.report(msg::experiment_not_enabled("class-modifiers"), t);
        }
        self.listener
            .begin_class_or_mixin_or_named_mixin_application_prelude(mixin_keyword);
        self.cursor.advance(); // `mixin`
        let mut const_keyword: Option<TokenSpan> = None;
        if self.peek_kind() == TokenKind::Keyword(Keyword::Const) {
            const_keyword = Some(self.peek_span());
            self.cursor.advance();
        }
        // Mixin names take the type-name face unconditionally
        // (visitMixinDeclaration, error_verifier.dart:1840 — mixin
        // declarations have no `=` mixin-application form). Reserved heads
        // carry the row like built-in ones (oracle `mixin if {}`: keyword
        // row + built-in row, both 1:7 len 2). ensure_identifier consumes
        // the keyword silently (no keyword-name gate arm for this context).
        if self.is_built_in_or_reserved_name_token(0) {
            self.report(
                msg::built_in_identifier_as_name(
                    msg::BuiltInNameKind::Type,
                    self.cursor.lexeme(self.peek_id()),
                ),
                self.peek_span(),
            );
        }
        let name =
            self.ensure_declaration_name(IdentifierContext::ClassOrMixinOrExtensionDeclaration);
        // Official parseMixin passes inDeclaration: true, allowsVariance: true
        // (parser_impl.dart:3412): the header accepts variance modifiers.
        let type_param = self.compute_type_param_or_arg_on_lt(true);
        type_param.parse_variables(self, true);
        self.listener
            .begin_mixin_declaration(begin, augment, base, mixin_keyword, name);

        // header: primary constructor (reported), on, implements.
        self.parse_primary_constructor_opt(DeclarationKind::Mixin, Some(name), const_keyword, true);
        let mut clauses = HeaderClauses::default();
        // on
        if self.peek_kind() == TokenKind::Keyword(Keyword::On) {
            let on = self.peek_span();
            self.cursor.advance();
            if augment.is_some() {
                self.report(msg::mixin_augmentation_has_on_clause(), on);
            }
            let mut count = 1usize;
            self.parse_single_type_required(msg::NamedTypeClause::On);
            while self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                self.parse_single_type_required(msg::NamedTypeClause::On);
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
            // skipUnexpectedTokenOpt (:3475, junk list `on`/`implements`/`{`):
            // a non-keyword token directly before a clause keyword or the
            // body `{` is reported and skipped — the identical() follower
            // check only hits keyword stringValues and the `{` token type,
            // same lexeme-face reasoning as the class recovery's :3187 list.
            let next = self.peek_kind();
            if !matches!(next, TokenKind::Keyword(_)) {
                let follower = self.cursor.look_kind(1);
                if matches!(follower, TokenKind::Keyword(Keyword::On | Keyword::Implements))
                    || follower == TokenKind::OpenCurly
                {
                    self.report(
                        msg::unexpected_token(self.cursor.lexeme(self.peek_id())),
                        self.peek_span(),
                    );
                    self.cursor.advance();
                }
            }
            // on — both the plain `on` arm and the misspelled
            // `extend`/`extends` arm (:3519 — lexeme-based gate, `extend` is
            // an identifier) feed `parseMixinOn` (:3555) and share the
            // duplicate/order checks below; the misspelled token itself is
            // the clause's `onKeyword` anchor (the `token.next!` read at
            // :3558).
            let mut on_keyword: Option<TokenSpan> = None;
            if self.peek_kind() == TokenKind::Keyword(Keyword::On) {
                on_keyword = Some(self.peek_span());
                self.cursor.advance();
            } else if is_keyword_or_identifier(self.peek_kind())
                && matches!(self.cursor.lexeme(self.peek_id()), "extend" | "extends")
            {
                on_keyword = Some(self.peek_span());
                self.report(msg::expected_instead("on"), self.peek_span());
                self.cursor.advance();
            }
            if let Some(on) = on_keyword {
                let mut count = 1usize;
                self.parse_single_type_required(msg::NamedTypeClause::On);
                while self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    self.parse_single_type_required(msg::NamedTypeClause::On);
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
            }
            // implements
            if self.peek_kind() == TokenKind::Keyword(Keyword::Implements) {
                let implements = self.peek_span();
                self.cursor.advance();
                let mut count = 1usize;
                self.parse_single_type_required(msg::NamedTypeClause::Implements);
                while self.peek_kind() == TokenKind::Comma {
                    self.cursor.advance();
                    self.parse_single_type_required(msg::NamedTypeClause::Implements);
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
            // `inline-class` (`enabledIn: 3.3`): an `extension type`
            // declaration reports `experiment_not_enabled` at the `type`
            // keyword below the boundary (`endExtensionTypeDeclaration`,
            // ast_builder.dart:1607).
            if !self.features.inline_class_enabled {
                self.report(msg::experiment_not_enabled("inline-class"), type_keyword);
            }
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
            // Extension names take the extension-name face
            // (visitExtensionDeclaration, error_verifier.dart:1124).
            if self.is_built_in_name_token(0) {
                self.report(
                    msg::built_in_identifier_as_name(
                        msg::BuiltInNameKind::Extension,
                        self.cursor.lexeme(self.peek_id()),
                    ),
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
        self.parse_primary_constructor_opt(DeclarationKind::Extension, name, const_keyword, true);

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
                // Official expectedInstead: anchored on the offending
                // clause keyword itself (parser_impl.dart:3713-3719), then
                // `token` advances to it so `computeType` reads the type
                // AFTER the clause keyword — `extends C` resolves `C` as a
                // type (the `undefined_class` row), not `extends` itself.
                self.report(msg::expected_instead("on"), at);
                self.cursor.advance();
            } else {
                // Official expectedAfterButGot: anchored on `token` — the
                // last consumed header token (the name/type-argument end),
                // not the junk it precedes (parser_impl.dart:3720-3728;
                // oracle test_missing_on_withBlock anchors the row on the
                // extension name).
                let anchor = self.last_consumed_anchor();
                self.report(msg::expected_after_but_got("on"), anchor);
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
            // Extension-type names take the extension-type-name face
            // (visitExtensionTypeDeclaration, error_verifier.dart:1150).
            if self.is_built_in_or_reserved_name_token(0) {
                self.report(
                    msg::built_in_identifier_as_name(
                        msg::BuiltInNameKind::ExtensionType,
                        self.cursor.lexeme(self.peek_id()),
                    ),
                    self.peek_span(),
                );
            }
            let n = self.peek_span();
            self.cursor.advance();
            n
        } else {
            // Reserved heads fail the identifier probe but still become the
            // name (ensure_declaration_name's keyword-name gate), and the
            // verifier face rides the keyword exactly as on the identifier
            // route (oracle `extension type if(int i) {}`: both rows at
            // 1:16 len 2, representation parsed cleanly).
            if self.is_built_in_or_reserved_name_token(0) {
                self.report(
                    msg::built_in_identifier_as_name(
                        msg::BuiltInNameKind::ExtensionType,
                        self.cursor.lexeme(self.peek_id()),
                    ),
                    self.peek_span(),
                );
            }
            self.ensure_declaration_name(IdentifierContext::ClassOrMixinOrExtensionDeclaration)
        };
        // The token right after the name, pre-type-parameters: the
        // verifier's synthesized empty formal list lands its synthetic `()`
        // at `name.next.charOffset` (`endExtensionTypeDeclaration` →
        // `_syntheticFormalParameterList(builder.name)` → `insertParens`,
        // ast_builder.dart:1590-1597, token_stream_rewriter.dart:29-59), so
        // the no-parentheses representation face anchors here, zero length.
        self.ext_type_name_next = Some(self.peek_span());
        let type_param = self.compute_type_param_or_arg_on_lt(false);
        type_param.parse_variables(self, false);
        self.listener
            .begin_extension_type_declaration(augment, extension_keyword, name);
        self.parse_primary_constructor_opt(
            DeclarationKind::ExtensionType,
            Some(name),
            const_keyword,
            augment.is_none(),
        );
        self.ext_type_name_next = None;

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
    /// below). `name` is the declaration's name span — only the Class junk
    /// walks use it (the `expected_body` anchor).
    fn parse_primary_constructor_opt(
        &mut self,
        kind: DeclarationKind,
        name: Option<TokenSpan>,
        const_keyword: Option<TokenSpan>,
        allow_representation: bool,
    ) -> bool {
        if kind == DeclarationKind::Class
            && name.is_some()
            && self.peek_kind() == TokenKind::OpenParen
            && !self.peek_is_possible_record_type()
            && matches!(
                self.cursor.look_kind(1),
                TokenKind::Semicolon | TokenKind::OpenSquare
            )
        {
            // The scanner-unpaired `(` with a `;`/`[` tail (`class C(;`,
            // `class C( ;`, `class C( [}`): upstream `parseFormalParametersRest`
            // (`parser_impl.dart:1916`) attempts the formal list, the junk
            // token takes the missing-first-formal-name face
            // (`parseFormalParameter` → `ensureIdentifier`), and the
            // never-closing group bails via the synthetic-closer move
            // (`:1985` — `begin.endGroup!.isSynthetic` →
            // `rewriter.moveSynthetic`); the unconsumed junk then re-parses
            // as class members (`parseClass` :3105-3114 → `ensureBlock` +
            // `parseClassOrMixinOrExtensionBody`), which is the face the
            // walks below reproduce.
            return self.recover_class_head_paren_junk(name.unwrap());
        }
        if kind == DeclarationKind::Class
            && self.peek_kind() == TokenKind::OpenParen
            && self.cursor.look_kind(1) == TokenKind::OpenCurly
            && self.class_head_paren_junk_tail()
        {
            // The probed junk shape: the head `(` never becomes a primary
            // constructor, so the oracle leaves it as skipped junk ahead of
            // the `{}`-remnant class body and the face is the junk walk
            // below. Two sub-shapes share it (3.13.7 probes): the
            // scanner-unpaired `(` (`class C( {}` — the formals can never
            // close, at any language version or feature state) and the
            // paired empty `({})` with no `;`/body `{` ahead (`class C({})`
            // at EOF — the oracle junks it, while `class C({}) ;`,
            // `class C({}) {}` and `class C({}) extends B {}` parse it as
            // real formals). A paired non-empty `(` (`class C({int a})`)
            // parses as primary-constructor formals.
            return self.recover_class_head_paren_off();
        }
        if self.peek_kind() == TokenKind::OpenParen || self.peek_kind() == TokenKind::Period {
            let begin = self.peek_span();
            // Official :3798-3823 — the per-kind face anchors on the `const`
            // keyword when present, else on the `(`/`.`; with the feature off
            // the plain unexpected-token report names the SAME token (oracle
            // invalid_primary_constructors_pre_feature 13|7 `mixin const
            // M() {}` → "Unexpected text 'const'.", 5|8 `mixin M() {}` →
            // "Unexpected text '('.").
            let anchor = const_keyword.unwrap_or(begin);
            match kind {
                DeclarationKind::Mixin => {
                    if self.features.primary_constructors_enabled {
                        self.report(msg::mixin_primary_constructor(), anchor);
                    } else {
                        self.report(msg::unexpected_token(self.cursor.lexeme(anchor.id)), anchor);
                    }
                }
                DeclarationKind::Extension => {
                    if self.features.primary_constructors_enabled {
                        self.report(msg::extension_primary_constructor(), anchor);
                    } else {
                        self.report(msg::unexpected_token(self.cursor.lexeme(anchor.id)), anchor);
                    }
                }
                DeclarationKind::ExtensionType if !allow_representation => {
                    self.report(
                        msg::extension_type_augmentation_specifies_representation_field(),
                        anchor,
                    );
                }
                _ => {}
            }

            self.listener.begin_primary_constructor(begin);
            let has_constructor_name = self.peek_kind() == TokenKind::Period;
            let constructor_name = if has_constructor_name {
                self.cursor.advance();
                Some(self.ensure_identifier(IdentifierContext::PrimaryConstructorDeclaration))
            } else {
                None
            };
            if self.peek_kind() == TokenKind::OpenParen {
                // Not augmentation-gated: the verifier runs its shape checks
                // whenever the constructor head parsed — the AST builder
                // keeps the PrimaryConstructorDeclaration for augmentations
                // too (ast_builder.dart:6484-6496), so a bad representation
                // shape reports on `augment extension type E(this.x)` the
                // same as on an introductory declaration.
                if kind == DeclarationKind::ExtensionType {
                    self.rep_field_check = Some(crate::parser::RepFieldCheck::default());
                }
                self.parse_formal_parameters_required_opt(
                    crate::listener::MemberKind::PrimaryConstructor,
                );
                let rep = self.rep_field_check.take();
                if let Some(state) = rep {
                    // `_checkForExtensionTypeRepresentationErrorCodes`
                    // (error_verifier.dart:5356-5460; primary constructors
                    // feature disabled arm). The verifier runs whenever the
                    // constructor parsed — augmentations included: the AST
                    // builder keeps the PrimaryConstructorDeclaration for
                    // any parsed `(`/`.` head (ast_builder.dart:6484-6496),
                    // so the shape checks are not augmentation-gated.
                    if state.count == 0 {
                        // `expectedRepresentationField` anchors on
                        // `formalParameterList.leftParenthesis.next`
                        // (error_verifier.dart:5371) — the `)` closing the
                        // empty list, never the body `{`.
                        self.report(
                            msg::expected_representation_field(),
                            self.last_consumed_anchor(),
                        );
                    } else if state.count > 1 {
                        if let Some(comma) = state.comma_after_first {
                            self.report(msg::multiple_representation_fields(), comma);
                        }
                    } else if let Some(kw) = state.this_or_super {
                        // The initializing-formal faces anchor on the
                        // `this`/`super` keyword itself
                        // (error_verifier.dart:5384-5393; oracle
                        // regress_53625 53|24 spans `this`).
                        self.report(msg::expected_representation_field(), kw);
                    } else if let Some(delim) = state.delimiter {
                        // The verifier reads the left delimiter only in the
                        // primary-constructors-disabled arm
                        // (error_verifier.dart:5427-5434); with the feature
                        // on a delimited form (e.g. `[int x]`) survives
                        // clean.
                        if !self.features.primary_constructors_enabled {
                            self.report(msg::expected_representation_field(), delim);
                        }
                    } else if state.fn_typed {
                        // The verifier's `expectedRepresentationField` for a
                        // function-typed representation field only runs with
                        // primary constructors disabled (the 3.13 oracle runs
                        // the feature and accepts the declaring parameter),
                        // and anchors on `inner.beginToken` — the formal's
                        // first token, the modifier or type, never the
                        // list's `(` (error_verifier.dart:5433-5439; oracle
                        // declaring_parameter_pre_feature 41|19 spans
                        // `var`).
                        if !self.features.primary_constructors_enabled {
                            self.report(
                                msg::expected_representation_field(),
                                state.first_begin.unwrap_or(begin),
                            );
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
                    // Official anchor is `token` — the constructor-name
                    // identifier when present (:3857-3862), so
                    // `class C<T>.named {}` anchors on `named`, never on the
                    // `{` follower; a junk name anchors on its synthetic
                    // (zero-width at the follower) exactly as
                    // `ensureIdentifier` returned it.
                    self.report(
                        msg::missing_primary_constructor_parameters(),
                        constructor_name.expect("period entry guarantees the name"),
                    );
                }
                if kind == DeclarationKind::ExtensionType {
                    // `endPrimaryConstructor` synthesizes the empty formal
                    // list for a parsed-but-parameterless constructor head
                    // (ast_builder.dart:2709-2712): `insertParens` places the
                    // synthetic `()` at the offset of the token after the
                    // type name — the `.` that opened the constructor — and
                    // the verifier's empty-list arm then anchors on the
                    // synthetic `)`, zero length. Not augmentation-gated:
                    // the constructor parsed, so the verifier runs
                    // (oracle Augmentations extension_types_A01_t02 19|27).
                    let at = TokenSpan {
                        range: SourceRange {
                            offset: begin.range.offset,
                            length: 0,
                        },
                        ..begin
                    };
                    self.report(msg::expected_representation_field(), at);
                }
                let at = self.peek_span();
                self.listener.handle_no_formal_parameters(
                    at,
                    crate::listener::MemberKind::PrimaryConstructor,
                );
            }
            // AstBuilder `endPrimaryConstructor`'s Class/Enum arm (the 3.13.1
            // oracle): a *parsed* primary constructor is gated at the AST
            // layer and reports `experimentNotEnabled` anchored on the begin
            // token (`(` or `.`) when the file's language version predates
            // the feature. The shared parser itself never gates the
            // Class/Enum formals parse, so `class C(int x);` at 2.12 stays a
            // clean parse whose whole face is this row plus parseClass's
            // `;`-body row. Mixin/Extension hit the `TopLevel|Mixin|Extension`
            // arm, which is silent here ("error reported in the parser") —
            // live 3.13.1 confirms `mixin M(int x);` at 2.12 emits only the
            // parser-side rows, no `(`-anchored experiment line.
            if !self.features.primary_constructors_enabled
                && matches!(kind, DeclarationKind::Class | DeclarationKind::Enum)
            {
                self.report_experiment_not_enabled(begin);
            }
            let end = self.peek_span();
            self.listener.end_primary_constructor(
                kind,
                begin,
                end,
                const_keyword,
                has_constructor_name,
                constructor_name,
            );
        } else {
            if kind == DeclarationKind::ExtensionType && allow_representation {
                // Both no-representation faces are introductory-only. The
                // parser's `missingPrimaryConstructor` is gated on
                // `allowExtensionTypeRepresentation` (official :3831-3834 —
                // augmentations carry no representation to be missing), and
                // the verifier never sees an augmentation here either: with
                // no parsed constructor head the AST builder leaves the
                // augmentation's namePart a NameWithTypeParameters — the
                // synthesized empty formal list is intro-only
                // (ast_builder.dart:1590-1597) — so the check returns before
                // the empty-list arm (error_verifier.dart:5360-5362).
                // Oracle: every `augment extension type E {}` row set is
                // silent on both codes (co19 Augmentations suite).
                // Official `reportRecoverableError` on the primary
                // constructor position token (parser_impl.dart:3833),
                // anchored per `findNonZeroLengthToken` (util.dart:56-61): a
                // real name/type-parameter tail anchors on itself (the
                // backward walk of [`Parser::last_consumed_anchor`]); a
                // zero-width synthetic name — annotation junk consumed at
                // the name position — walks FORWARD to the first real token
                // instead (the junk identifier the spliced name points at),
                // where the oracle anchors the row (Primary-constructors
                // grammar_A06_t01 45|17|4).
                let at = match self.cursor.last_consumed_id() {
                    Some(id) if self.span_of(id).range.length > 0 => self.last_consumed_anchor(),
                    _ => {
                        let mut at = self.peek_span();
                        let mut n = 0usize;
                        while at.range.length == 0 && self.cursor.look_kind(n) != TokenKind::Eof {
                            n += 1;
                            at = self.look_span_at(n);
                        }
                        at
                    }
                };
                self.report(msg::missing_primary_constructor(), at);
                // The verifier's synthesized empty formal list
                // (`_syntheticFormalParameterList(builder.name)` →
                // `insertParens`) gives the synthetic `()` the offset of the
                // token after the type name, zero length
                // (token_stream_rewriter.dart:38-59) — the empty-list arm
                // then anchors on the synthetic `)` there. The anchor is the
                // pre-type-parameters follower (oracle
                // no_primary_constructor 4|19 = the `<`), captured at the
                // name step by the extension-type caller.
                if let Some(next) = self.ext_type_name_next.take() {
                    let at = TokenSpan {
                        range: SourceRange {
                            offset: next.range.offset,
                            length: 0,
                        },
                        ..next
                    };
                    self.report(msg::expected_representation_field(), at);
                }
            } else if let Some(c) = const_keyword {
                if self.features.primary_constructors_enabled {
                    match kind {
                        DeclarationKind::Mixin => self.report(msg::mixin_primary_constructor(), c),
                        DeclarationKind::Extension => {
                            self.report(msg::extension_primary_constructor(), c);
                        }
                        _ => self.report(msg::const_without_primary_constructor(), c),
                    }
                } else {
                    // Official :3884-3905: with the feature disabled the
                    // per-kind faces collapse into the plain unexpected-token
                    // report on the `const` keyword, for every declaration
                    // kind (oracle invalid_primary_constructors_pre_feature
                    // 2|7, 3|6: `class const C {}` / `enum const E {}`).
                    self.report(msg::unexpected_token(self.cursor.lexeme(c.id)), c);
                }
            }
            let at = self.peek_span();
            self.listener
                .handle_no_primary_constructor(kind, at, const_keyword);
        }
        true
    }

    /// Whether the `(` + `{` at the cursor is the junk shape routed to
    /// [`Self::recover_class_head_paren_off`] (see the caller for the two
    /// sub-shapes). The paired case needs a bounded lookahead: `class
    /// C({}) extends B {}` (r06) is clean formals while `class C({})
    /// extends B` (t01) is junk — the discriminator is whether a `;` body
    /// or a real `{` body ever appears across the clause list.
    fn class_head_paren_junk_tail(&self) -> bool {
        if !self.peek_is_possible_record_type() {
            // The `(` has no real closer: the formals can never close, so
            // the shape is junk regardless of the language version or the
            // primary-constructors state.
            return true;
        }
        // Paired: only the empty `{}` named-section remnant is a junk
        // candidate (`class C({int a})` parses its formals).
        if self.cursor.look_kind(2) != TokenKind::CloseCurly {
            return false;
        }
        let mut clause = false;
        for n in 3..128 {
            match self.cursor.look_kind(n) {
                // n == 3 is the group's own `)` closer (`(`,`{`,`}`,`)`) —
                // part of the formals group, never a junk discriminator
                // (oracle probes: `class C({}) ;` and `class C({}) {}` parse
                // the formals; without this arm the closer fell into the
                // catch-all below and junked them).
                TokenKind::CloseParen if n == 3 => {}
                TokenKind::Semicolon | TokenKind::OpenCurly => return false,
                TokenKind::Eof => return true,
                TokenKind::Keyword(Keyword::Extends | Keyword::With | Keyword::Implements) => {
                    clause = true;
                }
                TokenKind::Identifier
                | TokenKind::Period
                | TokenKind::Comma
                | TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::Question
                | TokenKind::Ampersand
                    if clause => {}
                _ => return true,
            }
        }
        true
    }

    /// Junk walk for the class-head `(` + `{` shapes (`class C( {}`,
    /// `class C({})` at EOF) — the 3.13.7 oracle face, probe-pinned per
    /// shape. The full ordered face set: `unexpected_token` on the skipped
    /// `(` (`parser_impl.dart` skip face), `expected_token(';')` at the
    /// `{`, `missing_identifier` + `missing_const_final_var_or_type` at
    /// the `}` (the empty `{}`-remnant parses as a member with no type and
    /// no name), then `expected_executable` carried by the closer that
    /// follows the remnant — a scanner-parked synthetic `)` re-anchored
    /// forward over the zero-width walk (`findNonZeroLengthToken`,
    /// util.dart:56; EOF keeps the zero-width anchor) or the real `)`. The
    /// scanner's own unbalanced-group row (`lexer/mod.rs:908-917`) carries
    /// the `expected_token(')')` face; identical re-anchored rows collapse
    /// through the shared dedup (`admit_row`). The token after the closers
    /// re-parses at top level (the follower `void f() {}` keeps its own
    /// clean parse — observably identical to the oracle, which emits no
    /// rows for it either). Returns `false`: the tail was consumed.
    fn recover_class_head_paren_off(&mut self) -> bool {
        let lparen = self.peek_span();
        // The head `(` is scanner-unpaired on this route only when no `)`
        // follows anywhere (the caller's `{`-tail probe admits scanner-paired
        // empties too — `class C({})` at EOF); the pending-paren arm below
        // splits its faces on exactly this fact.
        let paren_unpaired = !self.peek_is_possible_record_type();
        // The scanner's parked synthetic closer for the unpaired head `(`:
        // `close_unclosed_groups` parks it at EOF (or `report_unmatched_begin`
        // before a mismatching closer) and records the pair without a
        // diagnostic, deferring the row to the parser's post-parse pass.
        let parked_close = if paren_unpaired {
            self.cursor.table_matching_end(lparen.id)
        } else {
            None
        };
        // `endPrimaryConstructor` (outline_builder.dart:1810-1818): the
        // Class/Enum arm runs `reportIfNotEnabled(primaryConstructors,
        // beginPrimaryConstructor)` whenever the `(` head parsed — the junk
        // route included (oracle `// @dart=2.12 class C( {}`: the
        // experiment row on the `(` precedes the junk face).
        if !self.features.primary_constructors_enabled {
            self.report_experiment_not_enabled(lparen);
        }
        let lparen_lexeme = self.cursor.lexeme(lparen.id).to_owned();
        self.report(msg::unexpected_token(&lparen_lexeme), lparen);
        self.cursor.advance(); // the skipped junk `(`
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
        // the walk drains to EOF without hitting the follower arm, the pair
        // stays unadopted, and the parse-end pass anchors the
        // `expected_token(')')` row at the EOF-parked synthetic
        // (`close_unclosed_groups`, lexer/mod.rs:1117-1152); the parser's
        // `expected_executable` rides that synthetic and keeps the
        // zero-width EOF anchor (2:1 0).
        // Follower path (`class C( {} void f() {}`): the walk stops at the
        // boundary and adopts the parked closer at the relocation point —
        // the `expected_token(')')` row lands on the boundary (oracle
        // `expected_executable` + `expected_token ')'` on `void`) and the
        // follower keeps a clean top-level parse.
        // With a real `)` the closer carries the row on its own span
        // (oracle `class C({})`: `expected_executable` at the `)`), and a
        // trailing stray `}` adds its own row (oracle `class C({}) }` rows
        // at both closers); the unpaired twin `class C( {} }` collapses to
        // one row at the stray `}` — the synthetic closer's re-anchored row
        // and the real brace's row share the anchor, and the shared dedup
        // (`unmatched_seen`) drops the second.
        let mut boundary_carried = false;
        while !self.cursor.at_eof() {
            if !matches!(
                self.peek_kind(),
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly
            ) {
                // Follower path with the `(` never closed (`class C( {} void
                // f() {}`, `class C( {}\nclass D {}`): the oracle's body
                // recovery still opens a remnant member at the boundary —
                // `expected_executable` on the follower token
                // (class_member_parser junk-member face) — and the follower
                // itself re-parses cleanly at top level. When a closer
                // precedes the follower, that closer already carried the
                // boundary face and the follower stays silent (oracle
                // `class C( {} } void f() {}`: the face at the stray `}`,
                // nothing on `void`). Paired shapes (`class C({}) x`) carry
                // the face on their real closers instead, so the arm is
                // gated on the head paren's own pairing, not the
                // `{`-remnant's closer.
                if paren_unpaired {
                    // moveSynthetic relocation (`ensureCloseParen`'s
                    // `begin.endGroup!.isSynthetic` bailout,
                    // parser_impl.dart:1985 via :5028-5037) — eof-parked
                    // closers only (oracle `class C( {} void f() {}` → `)`
                    // 1:13 on `void`; `class C( {} } void f() {}` keeps the
                    // stray-`}` park), see
                    // [`Self::adopt_paren_at_relocation`].
                    self.adopt_paren_at_relocation(lparen, parked_close, self.peek_span());
                    if !boundary_carried {
                        self.report(msg::expected_executable(), self.find_non_zero_span());
                    }
                }
                break;
            }
            self.report(msg::expected_executable(), self.find_non_zero_span());
            boundary_carried = true;
            self.cursor.advance();
        }
        false
    }

    /// Batch-B relocation adoption, gated to the EOF-parked pending `(`: the
    /// formals-bail `moveSynthetic` (`parser_impl.dart:1985` via
    /// `:5028-5037`) re-anchors only an eof-parked closer at the resume
    /// point; a closer parked at a mismatching bracket stays put (oracle
    /// `class C( {} } void f() {}` and `class C( [x] }`: `)` at the stray
    /// `}`, not the follower). Reports the `expected_token ')'` row at
    /// `at`, latches [`Parser::unmatched_reported`] so the parse-end pass
    /// skips the pair, and hides the parked synthetic.
    fn adopt_paren_at_relocation(
        &mut self,
        lparen: TokenSpan,
        parked: Option<crate::token_id::TokenId>,
        at: TokenSpan,
    ) {
        let Some(parked) = parked else {
            return;
        };
        if self.span_of(parked).range.offset < self.table.last().utf16_offset {
            return;
        }
        self.report_unmatched_row(")", at);
        self.unmatched_reported.push(lparen.id);
        self.cursor.hide(parked);
    }

    /// `findNonZeroLengthToken` (`parser/util.dart:56`) from the cursor:
    /// the first token at or after the current position carrying a
    /// non-zero length; a zero-width anchor with no non-zero successor
    /// falls through to the EOF token's own span (the zero-width anchor
    /// survives at EOF).
    fn find_non_zero_span(&self) -> TokenSpan {
        let mut n = 0usize;
        while !matches!(self.cursor.look_kind(n), TokenKind::Eof) {
            let span = self.look_span_at(n);
            if span.range.length > 0 {
                return span;
            }
            n += 1;
        }
        self.look_span_at(n)
    }

    /// Junk walk for the scanner-unpaired class-head `(` with a `;`/`[` tail
    /// (`class C(;`, `class C( ;`, `class C( [}`) — the 3.13.7 oracle face,
    /// probe-pinned per shape. Upstream route: `parseFormalParametersRest`
    /// (`parser_impl.dart:1916`) reports the missing first-formal name over
    /// the junk token (`parseFormalParameter` → `ensureIdentifier`) and
    /// bails the never-closing group at `:1985` (`begin.endGroup!.isSynthetic`
    /// → `rewriter.moveSynthetic` — the scanner's synthetic `)` stays for the
    /// post-parse unmatched-group row); `parseClass` (:3105-3114) then runs
    /// `ensureBlock` (the `expected_body` row on the class name) and the body
    /// loop, where the junk re-parses as members: the `(` head and a `[`
    /// opener take the junk-member `expected_executable` face, the `;`
    /// remnant takes the field-remnant faces (`missing_identifier` +
    /// `expected_token(';')` + `unexpected_token`, all on the `;`), and the
    /// next remnant member takes `expected_executable` +
    /// `missing_const_final_var_or_type` at the tail (EOF keeps the
    /// zero-width anchor; a real follower anchors on itself, oracle `class
    /// C(; void f() {}` rows on `void`). The eof-parked pending `(` adopts
    /// at a follower ([`Self::adopt_paren_at_relocation`]); the
    /// bracket's park-position row keeps flowing through the un-latched
    /// scanner pair, with the relocation/EOF row added alongside
    /// (`]` at a mismatching closer AND on the follower / at EOF). Returns
    /// `false`: the tail was consumed.
    fn recover_class_head_paren_junk(&mut self, name: TokenSpan) -> bool {
        // `endPrimaryConstructor` (outline_builder.dart:1810-1818): the
        // experiment face fires for Class whenever the head parsed — the
        // formals bail does not skip the listener end event.
        if !self.features.primary_constructors_enabled {
            self.report_experiment_not_enabled(self.peek_span());
        }
        // `ensureBlock` (parser_impl.dart:3110) — the class never grows a
        // body, and the oracle anchors `expected_body` on the name. The
        // per-kind block-body message keeps the analyzer registry name
        // `expected_body` (message.rs `expected_block_body`).
        self.report(
            msg::expected_block_body(msg_block_kind_class(), ""),
            name,
        );
        let lparen = self.peek_span();
        let paren_parked = self.cursor.table_matching_end(lparen.id);
        self.report(msg::expected_executable(), lparen);
        self.cursor.advance(); // the junk `(` head
        if self.peek_kind() == TokenKind::OpenSquare {
            // `[` junk member: the head face plus the field-terminator face
            // (oracle `class C( [}`: `expected_executable` + `expected ';'`
            // both on the `[`), then the `}` remnant member with the
            // unclosed-section closer face. The scanner splices its
            // synthetic `]` at the mismatching `}`'s offset
            // (`report_unmatched_begin`), so the remnant faces re-anchor
            // over it onto the real `}` (`findNonZeroLengthToken`,
            // parser/util.dart:56) before both closers are consumed.
            let open_sq = self.peek_span();
            let bracket_synthetic = self
                .cursor
                .table_matching_end(open_sq.id)
                .is_some_and(|end| self.cursor.resolve(end).1);
            self.report(msg::expected_executable(), open_sq);
            self.report(msg::expected_token(";"), open_sq);
            self.cursor.advance();
            let remnant = self.find_non_zero_span();
            self.report(msg::expected_executable(), remnant);
            self.report(msg::missing_identifier(), remnant);
            self.report(msg::missing_const_final_var_or_type(), remnant);
            while matches!(
                self.peek_kind(),
                TokenKind::CloseSquare | TokenKind::CloseCurly
            ) {
                self.cursor.advance();
            }
            // The `}` remnant already carried the field faces. The bracket
            // family closes at EOF whether or not a follower follows: the
            // oracle anchors the tail member and the pending-group rows at
            // the eof position (oracle `class C( [}`: 1|12|0 + 1|12|1 x2;
            // the twin with a follower renders the same rows on the
            // newline's line, `class C( [} void f() {}`: 2|1|0 + 2|1|1 x2)
            // — never on the follower. The pending `(` keeps its scanner
            // park (post-parse row; `adopt_paren_at_relocation` is the
            // semicolon arm's law, not this one).
            let tail = self.find_non_zero_span();
            let eof_off = self.table.last().utf16_offset;
            let at_eof = tail.range.offset >= eof_off;
            let eof_span = TokenSpan::new(
                (self.table.len() - 1) as crate::token_id::TokenId,
                SourceRange {
                    offset: eof_off,
                    length: 0,
                },
            );
            if bracket_synthetic {
                // The unmatched `[` rows ride two channels at once: the
                // un-latched scanner pair keeps the park-position row
                // (post-parse `]` at the mismatching `}`), and the
                // relocation/EOF channel adds one more — on the follower's
                // newline line with a follower (oracle `class C( [} void
                // f() {}`: 2|1|1 beside the 1|11|1 park row), at EOF for
                // the drain (oracle `class C( [}`: 1|12|1; the `class C( [`
                // drain dedups into the eof-parked pair's row). Latching
                // the pair would kill the park row the oracle keeps.
                if at_eof {
                    self.report_unmatched_row("]", tail);
                } else {
                    self.report_unmatched_row("]", eof_span);
                }
            }
            if at_eof {
                // The `class C( [` drain anchors the remnant faces at EOF
                // too — the tail member IS the remnant, so the bare
                // `expected_executable` would double-report (oracle
                // `class C( [`: one row per face).
                if tail.range.offset != remnant.range.offset {
                    self.report(msg::expected_executable(), tail);
                }
            } else {
                self.report(msg::expected_executable(), eof_span);
            }
        } else {
            // `;` remnant member: the field-remnant faces all land on the
            // `;` (oracle `class C(;`: `missing_identifier`,
            // `expected_token(';')`, `unexpected_token` — one row each, all
            // width 1 on the `;`).
            let semi = self.peek_span();
            self.report(msg::missing_identifier(), semi);
            self.report(msg::expected_token(";"), semi);
            self.report(msg::unexpected_token(self.cursor.lexeme(semi.id)), semi);
            self.cursor.advance();
            // The next remnant member opens at the tail (EOF keeps the
            // zero-width anchor; a real follower anchors on itself), and
            // the eof-parked pending `(` relocates to the follower (oracle
            // `class C(; void f() {}`: `)` 1|11|1 on `void`; the EOF
            // variant keeps the parked row, re-emitted through the shared
            // dedup).
            let tail = self.find_non_zero_span();
            self.adopt_paren_at_relocation(lparen, paren_parked, tail);
            self.report(msg::expected_executable(), tail);
            self.report(msg::missing_const_final_var_or_type(), tail);
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
/// `augment`/`final_kw` are consumed by the declaration families; the rest
/// are first-token records driving the duplicate recovery (the second
/// occurrence reports `duplicated_modifier` and stays unrecorded).
#[derive(Default)]
pub(crate) struct DeclModifiers {
    pub(crate) augment: Option<TokenSpan>,
    pub(crate) final_kw: Option<TokenSpan>,
    const_kw: Option<TokenSpan>,
    external: Option<TokenSpan>,
    abstract_kw: Option<TokenSpan>,
    covariant: Option<TokenSpan>,
    late_kw: Option<TokenSpan>,
    required: Option<TokenSpan>,
    static_kw: Option<TokenSpan>,
    var_kw: Option<TokenSpan>,
}

/// Modifier tokens captured by the class-modifier walk. Unlisted modifiers
/// are first-token records for the post-walk faces and the duplicate
/// recovery (the second occurrence reports `duplicated_modifier` and stays
/// unrecorded).
#[derive(Default)]
struct ClassModifiers {
    abstract_kw: Option<TokenSpan>,
    augment: Option<TokenSpan>,
    const_kw: Option<TokenSpan>,
    covariant: Option<TokenSpan>,
    external: Option<TokenSpan>,
    final_kw: Option<TokenSpan>,
    late_kw: Option<TokenSpan>,
    required: Option<TokenSpan>,
    static_kw: Option<TokenSpan>,
    var_kw: Option<TokenSpan>,
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
