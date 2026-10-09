//! Formal parameter grammar (official `parseFormalParametersRequiredOpt`
//! `parser_impl.dart:1913`, `parseFormalParametersRest` :1936,
//! `parseFormalParameter` :2049, `parseOptionalPositionalParameters` :2418,
//! `parseOptionalNamedParameters` :2482).
//!
//! Full D7/D8 shape: required positional parameters, `[`/`{` optional
//! groups, `required`/`covariant`/`var`/`final` modifier validation,
//! initializing-formal (`this.x`/`super.x`) parameters, inline function-typed
//! parameters, and default values. Default value *expressions* stay on the
//! reduced constant-expression engine until #20 (documented reduction; the
//! official parses them with the full expression engine).

use dartq_scanner::{Keyword, TokenKind};

use crate::listener::{FormalParameterKind, IdentifierContext, MemberKind, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::recovery::is_identifier;
use crate::token_id::TokenSpan;
use crate::type_args::TypeParamOrArg;

impl<L: ParserListener> Parser<'_, '_, L> {
    /// Official `parseFormalParametersRequiredOpt`: a required `(...)` group;
    /// missing `(` is recovered with a synthetic empty list.
    pub(crate) fn parse_formal_parameters_required_opt(&mut self, kind: MemberKind) -> TokenSpan {
        if self.peek_kind() != TokenKind::OpenParen {
            // Recovery: synthesize an empty parameter list.
            let at = self.peek_span();
            self.report(msg::missing_parameters(kind), at);
            let open = self.synthetic(TokenKind::OpenParen, at.range.offset);
            let close = self.synthetic(TokenKind::CloseParen, at.range.offset);
            self.listener.begin_formal_parameters(open, kind);
            self.listener.end_formal_parameters(0, open, close, kind);
            return close;
        }
        self.parse_formal_parameters_rest(kind)
    }

    /// Official `parseFormalParametersRest`; the cursor sits before `(`.
    ///
    /// Depth-guarded: inline function-typed parameters recurse back into this
    /// function through `parseFormalParameter`, so `((((...` in a parameter
    /// list is unbounded recursion without the guard (official
    /// `recoverFromStackOverflow`).
    pub(crate) fn parse_formal_parameters_rest(&mut self, kind: MemberKind) -> TokenSpan {
        if !self.enter_depth() {
            return self.peek_span();
        }
        let begin = self.peek_span();
        let begin_group_close = self.current_matching_end();
        self.listener.begin_formal_parameters(begin, kind);
        self.cursor.advance();
        let mut parameter_count = 0usize;
        let end;
        loop {
            if self.is_bailed() {
                end = self.peek_span();
                break;
            }
            match self.peek_kind() {
                TokenKind::CloseParen => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                TokenKind::OpenSquare => {
                    let after = self.parse_optional_positional_parameters(kind);
                    end = self.ensure_close_paren(after);
                    break;
                }
                TokenKind::OpenCurly => {
                    let after = self.parse_optional_named_parameters(kind);
                    end = self.ensure_close_paren(after);
                    break;
                }
                TokenKind::Index => {
                    // Recovery: `[]` as one token splits into `[` `]` (the
                    // scanner keeps Index unsplit; official
                    // `rewriteSquareBrackets`).
                    let at = self.peek_span();
                    // `[]` is atomic: consume it, queue the synthetic `]`
                    // plus the empty-parameter identifier, and parse that.
                    self.cursor.advance();
                    let name_id = self
                        .cursor
                        .insert_synthetic(TokenKind::Identifier, at.range.offset + 1);
                    self.cursor.advance();
                    let _ = name_id;
                    let close_id = self
                        .cursor
                        .insert_synthetic(TokenKind::CloseSquare, at.range.offset + 1);
                    self.parse_formal_parameter(FormalParameterKind::OptionalPositional, kind);
                    self.listener.begin_optional_formal_parameters(at);
                    self.listener.end_optional_formal_parameters(
                        1,
                        at,
                        self.span_of(close_id),
                        kind,
                    );
                    self.cursor.advance(); // synthetic `]`
                    end = self.ensure_close_paren(at);
                    break;
                }
                _ => {}
            }
            parameter_count += 1;
            self.parse_formal_parameter(FormalParameterKind::RequiredPositional, kind);
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseParen => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                _ => {
                    // Recovery: scanner-synthetic closers are adopted
                    // silently; id-id suggests a missing comma (reported,
                    // tokens kept); otherwise jump to the group closer.
                    end = self.recover_parameter_group_end(begin, begin_group_close);
                    break;
                }
            }
        }
        self.listener
            .end_formal_parameters(parameter_count, begin, end, kind);
        self.leave_depth();
        end
    }

    /// The `parseFormalParametersRest` tail recovery: a scanner-synthetic
    /// `)` is adopted (official `moveSynthetic`), `id id` reports a missing
    /// comma and continues, everything else jumps to the group's closer.
    fn recover_parameter_group_end(
        &mut self,
        begin: TokenSpan,
        begin_group_close: Option<crate::token_id::TokenId>,
    ) -> TokenSpan {
        if let Some(close) = begin_group_close
            && self.cursor.resolve(close).1
        {
            self.skip_to_token(close);
            let end = self.peek_span();
            self.cursor.advance();
            return end;
        }
        let next = self.cursor.peek();
        let next_next = self.cursor.look_kind(1);
        if is_identifier(next.kind) && is_identifier(next_next) {
            self.report(msg::expected_but_got(","), next.span);
            return self.peek_span(); // caller re-enters the loop
        }
        self.ensure_close_paren(begin)
    }

    /// Official `parseOptionalPositionalParameters`: `[`-bracketed group.
    fn parse_optional_positional_parameters(&mut self, kind: MemberKind) -> TokenSpan {
        let begin = self.peek_span(); // `[`
        let close_id = self.current_matching_end();
        self.cursor.advance();
        self.listener.begin_optional_formal_parameters(begin);
        let mut parameter_count = 0usize;
        let end;
        loop {
            if self.is_bailed() {
                end = self.peek_span();
                break;
            }
            if self.peek_kind() == TokenKind::CloseSquare {
                end = self.peek_span();
                break;
            }
            self.parse_formal_parameter(FormalParameterKind::OptionalPositional, kind);
            parameter_count += 1;
            if self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                continue;
            }
            if self.peek_kind() != TokenKind::CloseSquare {
                // Recovery: report and jump to the group's closer (the
                // scanner guarantees it).
                self.report(msg::expected_but_got("]"), self.peek_span());
                if let Some(close) = close_id {
                    self.skip_to_token(close);
                }
            }
            end = self.peek_span();
            break;
        }
        if parameter_count == 0 {
            let at = self.peek_span();
            self.report(msg::empty_optional_parameter_list(), at);
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, at.range.offset);
            self.cursor.advance();
            let _ = id; // synthetic name of the synthesized parameter
            self.parse_formal_parameter(FormalParameterKind::OptionalPositional, kind);
        }
        self.cursor.advance(); // `]`
        self.listener
            .end_optional_formal_parameters(parameter_count, begin, end, kind);
        end
    }

    /// Official `parseOptionalNamedParameters`: `{`-bracketed group.
    fn parse_optional_named_parameters(&mut self, kind: MemberKind) -> TokenSpan {
        let begin = self.peek_span(); // `{`
        let close_id = self.current_matching_end();
        self.cursor.advance();
        self.listener.begin_optional_formal_parameters(begin);
        let mut parameter_count = 0usize;
        let end;
        loop {
            if self.is_bailed() {
                end = self.peek_span();
                break;
            }
            if self.peek_kind() == TokenKind::CloseCurly {
                end = self.peek_span();
                break;
            }
            self.parse_formal_parameter(FormalParameterKind::OptionalNamed, kind);
            parameter_count += 1;
            if self.peek_kind() == TokenKind::Comma {
                self.cursor.advance();
                continue;
            }
            if self.peek_kind() != TokenKind::CloseCurly {
                // Recovery: report and jump to the group's closer (the
                // scanner guarantees it).
                self.report(msg::expected_but_got("}"), self.peek_span());
                if let Some(close) = close_id {
                    self.skip_to_token(close);
                }
            }
            end = self.peek_span();
            break;
        }
        if parameter_count == 0 {
            let at = self.peek_span();
            self.report(msg::empty_named_parameter_list(), at);
            let id = self
                .cursor
                .insert_synthetic(TokenKind::Identifier, at.range.offset);
            self.cursor.advance();
            let _ = id;
            self.parse_formal_parameter(FormalParameterKind::OptionalNamed, kind);
        }
        self.cursor.advance(); // `}`
        self.listener
            .end_optional_formal_parameters(parameter_count, begin, end, kind);
        end
    }

    /// Official `parseFormalParameter` (full shape).
    fn parse_formal_parameter(
        &mut self,
        mut parameter_kind: FormalParameterKind,
        member_kind: MemberKind,
    ) {
        self.parse_metadata_star();
        let start = self.peek_span();

        let in_function_type = member_kind == MemberKind::GeneralizedFunctionType;

        let mut required: Option<TokenSpan> = None;
        let mut covariant: Option<TokenSpan> = None;
        let mut var_final_or_const: Option<TokenSpan> = None;
        // Modifier walk (official parseFormalParameter head +
        // `parseFormalParameterModifiers` recovery).
        loop {
            match self.peek_kind() {
                TokenKind::Keyword(Keyword::Required)
                    if parameter_kind == FormalParameterKind::OptionalNamed =>
                {
                    parameter_kind = FormalParameterKind::RequiredNamed;
                    let t = self.peek_span();
                    self.cursor.advance();
                    required.get_or_insert(t);
                }
                TokenKind::Keyword(Keyword::Covariant)
                    if !matches!(
                        member_kind,
                        MemberKind::StaticMethod
                            | MemberKind::TopLevelMethod
                            | MemberKind::ExtensionNonStaticMethod
                            | MemberKind::ExtensionStaticMethod
                            | MemberKind::ExtensionTypeNonStaticMethod
                            | MemberKind::ExtensionTypeStaticMethod
                            | MemberKind::PrimaryConstructor
                    ) =>
                {
                    covariant.get_or_insert(self.peek_span());
                    self.cursor.advance();
                }
                TokenKind::Keyword(Keyword::Var | Keyword::Final) if !in_function_type => {
                    if var_final_or_const.is_some() {
                        let t = self.peek_span();
                        self.report(msg::duplicated_modifier(self.cursor.lexeme(t.id)), t);
                    } else {
                        var_final_or_const = Some(self.peek_span());
                    }
                    self.cursor.advance();
                }
                TokenKind::Keyword(Keyword::Const)
                    if !in_function_type && var_final_or_const.is_none() =>
                {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("const"), t);
                }
                TokenKind::Keyword(Keyword::Required) => {
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("required"), t);
                }
                _ => break,
            }
        }
        if member_kind == MemberKind::GeneralizedFunctionType
            && let Some(t) = var_final_or_const
        {
            self.report(msg::function_type_default_value(), t);
        }
        self.listener.begin_formal_parameter(
            start,
            member_kind,
            required,
            covariant,
            var_final_or_const,
        );

        // Type (required in function types, optional otherwise).
        let type_info = self.compute_type(in_function_type, false);
        let mut this_keyword: Option<TokenSpan> = None;
        let mut super_keyword: Option<TokenSpan> = None;
        let mut period_after_this_or_super: Option<TokenSpan> = None;
        let mut name_context = IdentifierContext::FormalParameterDeclaration;

        let before_this = self.cursor.mark();
        if !in_function_type
            && matches!(
                self.peek_kind(),
                TokenKind::Keyword(Keyword::This | Keyword::Super)
            )
        {
            let is_this = self.peek_kind() == TokenKind::Keyword(Keyword::This);
            let keyword = self.peek_span();
            self.cursor.advance();
            if self.peek_kind() == TokenKind::Period {
                let period = self.peek_span();
                self.cursor.advance();
                period_after_this_or_super = Some(period);
                name_context = IdentifierContext::FieldInitializer;
                if is_this {
                    this_keyword = Some(keyword);
                } else {
                    super_keyword = Some(keyword);
                }
            } else if matches!(
                self.peek_kind(),
                TokenKind::CloseParen
                    | TokenKind::Comma
                    | TokenKind::CloseSquare
                    | TokenKind::CloseCurly
                    | TokenKind::Eq
                    | TokenKind::Colon
                    | TokenKind::Eof
            ) {
                // `this`/`super` as a plain (invalid) parameter name: rewind
                // and let the type/name parse below consume it.
                self.cursor.reset(before_this);
            } else {
                // Missing `.` after this/super: report + synthesize.
                let at = self.peek_span();
                self.report(msg::expected_but_got("."), at);
                let id = self
                    .cursor
                    .insert_synthetic(TokenKind::Period, at.range.offset);
                self.cursor.advance();
                period_after_this_or_super = Some(self.span_of(id));
                name_context = IdentifierContext::FieldInitializer;
                if is_this {
                    this_keyword = Some(keyword);
                } else {
                    super_keyword = Some(keyword);
                }
            }
        }

        // Inline function-typed parameter detection: `name` followed by
        // `<...>` `(` or directly `(`.
        let mut before_inline_function_type: Option<TokenSpan> = None;
        let mut type_param = TypeParamOrArg::None;
        if self.peek_kind() == TokenKind::Lt {
            type_param = self.compute_type_param_or_arg_on_lt(false);
            if !matches!(type_param, TypeParamOrArg::None) {
                let closer = type_param.skip(self.peek_id());
                if self.kind_at(self.offset_of(closer) + 1) == TokenKind::OpenParen {
                    if let Some(t) = var_final_or_const
                        && member_kind != MemberKind::PrimaryConstructor
                    {
                        self.report(msg::function_type_default_value(), t);
                    }
                    before_inline_function_type = Some(self.peek_span());
                }
            }
        } else if self.peek_kind() == TokenKind::OpenParen && this_keyword.is_none() {
            if let Some(t) = var_final_or_const
                && member_kind != MemberKind::PrimaryConstructor
            {
                self.report(msg::function_type_default_value(), t);
            }
            before_inline_function_type = Some(self.peek_span());
        }

        let mut var_or_final = None;
        if let Some(t) = var_final_or_const {
            let is_var = self.cursor.lexeme(t.id) == "var";
            let is_final = self.cursor.lexeme(t.id) == "final";
            if (is_var || is_final) && member_kind != MemberKind::PrimaryConstructor {
                self.report(msg::extraneous_modifier(self.cursor.lexeme(t.id)), t);
            }
            if is_var || is_final {
                var_or_final = Some(t);
            }
        }

        if let Some(before) = before_inline_function_type {
            // The `<...>` variables were already computed above when the
            // shape started with `<`; parse them now.
            self.listener.begin_function_typed_formal_parameter(before);
            if !matches!(type_param, TypeParamOrArg::None) {
                type_param.parse_variables(self, false);
            }
            type_info.parse_type(self);
            let after_params =
                self.parse_formal_parameters_required_opt(MemberKind::FunctionTypedParameter);
            let mut question = None;
            if self.peek_kind() == TokenKind::Question {
                let q = self.peek_span();
                self.cursor.advance();
                question = Some(q);
            }
            self.listener
                .end_function_typed_formal_parameter(before, question);
            let _ = after_params;
            if in_function_type {
                self.report(msg::invalid_inline_function_type(), before);
            }
        } else if in_function_type {
            type_info.ensure_type_or_void(self);
        } else {
            type_info.parse_type(self);
        }

        // Name.
        let name = if in_function_type
            && !matches!(
                parameter_kind,
                FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
            )
            && !is_identifier(self.peek_kind())
            && before_inline_function_type.is_none()
        {
            let next = self.peek_span();
            self.listener.handle_no_name(next);
            next
        } else {
            self.ensure_identifier(name_context)
        };

        // Default value.
        let mut initializer_start = None;
        let mut initializer_end = None;
        match self.peek_kind() {
            TokenKind::Eq | TokenKind::Colon => {
                let equals = self.peek_span();
                let separator = self.peek_kind();
                self.cursor.advance();
                self.listener
                    .begin_formal_parameter_default_value_expression(equals);
                initializer_start = Some(self.peek_span());
                self.parse_expression();
                initializer_end = Some(self.peek_span());
                self.listener
                    .end_formal_parameter_default_value_expression(self.peek_span());
                self.listener.handle_valued_formal_parameter(
                    equals,
                    initializer_end.unwrap(),
                    parameter_kind,
                );
                if parameter_kind == FormalParameterKind::RequiredPositional {
                    self.report(msg::required_parameter_with_default(), equals);
                } else if parameter_kind == FormalParameterKind::OptionalPositional
                    && separator == TokenKind::Colon
                {
                    self.report(msg::positional_parameter_with_equals(), equals);
                }
            }
            _ => {
                let next = self.peek_span();
                self.listener.handle_formal_parameter_without_value(next);
            }
        }
        self.listener.end_formal_parameter(
            var_or_final,
            this_keyword,
            super_keyword,
            period_after_this_or_super,
            name,
            initializer_start,
            initializer_end,
            parameter_kind,
            member_kind,
        );
    }
}
