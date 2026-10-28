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
use dartq_source::SourceRange;

use crate::listener::{FormalParameterKind, IdentifierContext, MemberKind, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::recovery::is_identifier;
use crate::token_id::TokenSpan;
use crate::type_args::TypeParamOrArg;
use crate::types::TypeInfo;

impl<L: ParserListener> Parser<'_, '_, '_, L> {
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
            // Official `insertParens` splices the pair into the stream and
            // `parseFormalParametersRest` returns the inserted `)` — the
            // parser resumes AFTER the synthesized group, so the caller's
            // body-section reads see the real follower. Drain the queued
            // synthetics here (`parseGetterOrFormalParameters` recovery
            // drains the same way); leaving them queued feeds the synthetic
            // `(` to the body grammar and mints spurious rows (oracle
            // getter_in_function: `int get g => 1;` in a function body keeps
            // the `=>` for the expression-body parse).
            self.cursor.advance();
            self.cursor.advance();
            return close;
        }
        self.parse_formal_parameters_rest(kind)
    }

    /// Official `parseFormalParametersRest` (3.13.7 checkout:
    /// `parser_impl.dart:1916`); the cursor sits before `(`.
    ///
    /// Guarded by [`crate::parser::TYPE_DEPTH_CRASH_GUARD`], not
    /// [`crate::parser::MAX_DEPTH`]: nested function-typed parameters recurse
    /// back into this function through `parseFormalParameter` (:2029) →
    /// `parseFormalParametersRequiredOpt` (:2307), and the official parser
    /// carries NO counter on that chain — the only depth counters in
    /// `parser_impl.dart` are `statementDepth` (:6686) and `expressionDepth`
    /// (:6915). No observable cap may sit inside the oracle-defined domain
    /// (deep-but-valid function-typed formals parse silently); the crash
    /// guard only keeps the Rust recursion bounded far above any
    /// oracle-reachable depth.
    pub(crate) fn parse_formal_parameters_rest(&mut self, kind: MemberKind) -> TokenSpan {
        if !self.enter_type_depth() {
            return self.peek_span();
        }
        let begin = self.peek_span();
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
                    if kind == MemberKind::PrimaryConstructor {
                        let span = self.peek_span();
                        if let Some(state) = self.rep_field_check.as_mut()
                            && state.delimiter.is_none()
                        {
                            state.delimiter = Some(span);
                        }
                    }
                    // Official two-token shape `ensureCloseParen(token, begin)`
                    // (:1951): the report anchors on the token after the
                    // group, but the jump target is the LIST's `(` endGroup —
                    // staying in-list across any junk (including whole
                    // malformed groups that follow) instead of synthesizing
                    // a `)` that orphans the rest of the list.
                    self.parse_optional_positional_parameters(kind);
                    end = self.ensure_close_paren(begin);
                    break;
                }
                TokenKind::OpenCurly => {
                    if kind == MemberKind::PrimaryConstructor {
                        let span = self.peek_span();
                        if let Some(state) = self.rep_field_check.as_mut()
                            && state.delimiter.is_none()
                        {
                            state.delimiter = Some(span);
                        }
                    }
                    // `ensureCloseParen(token, begin)` (:1956) — jump to the
                    // list `(` endGroup, same rationale as the `[` arm.
                    self.parse_optional_named_parameters(kind);
                    end = self.ensure_close_paren(begin);
                    break;
                }
                TokenKind::Index => {
                    // Recovery: `[]` as one token splits into `[` `]` (the
                    // scanner keeps Index unsplit; official
                    // `rewriteSquareBrackets`).
                    let at = self.peek_span();
                    // `[]` is atomic: consume it, then queue the split
                    // faces — the identifier FIRST, so it is the current
                    // token when `parse_formal_parameter` runs. Official
                    // `rewriteAndRecover` (:2452-2462) inserts the
                    // zero-length identifier into the stream SILENTLY and
                    // `parseFormalParameter` consumes it as the formal's
                    // name; advancing past it here would feed the split `]`
                    // to the name step instead (`missing_identifier` and a
                    // derailed list).
                    self.cursor.advance();
                    self.cursor
                        .insert_synthetic(TokenKind::Identifier, at.range.offset + 1);
                    let close_id = self
                        .cursor
                        .insert_synthetic(TokenKind::CloseSquare, at.range.offset + 1);
                    // The split group is always empty, so
                    // `parseOptionalPositionalParameters` takes its
                    // `parameterCount == 0` arm (:2452-2462): report at the
                    // split `]` (`rewriteAndRecover` anchors on
                    // `token.next!`) before the parameter parse. The
                    // official split face is a real width-1 token
                    // (`rewriteSquareBrackets` :5134-5140), so the anchor
                    // spans one unit even though the queued `]` handle is a
                    // zero-width recovery synthetic.
                    let close_span = TokenSpan::new(
                        close_id,
                        SourceRange {
                            offset: at.range.offset + 1,
                            length: 1,
                        },
                    );
                    // The arm's code is the pseudo-shared `missingIdentifier`
                    // (fe91 codes :577-590, measured f14).
                    self.report(msg::missing_identifier(), close_span);
                    self.parse_formal_parameter(FormalParameterKind::OptionalPositional, kind);
                    self.listener.begin_optional_formal_parameters(at);
                    self.listener.end_optional_formal_parameters(
                        1,
                        at,
                        self.span_of(close_id),
                        kind,
                    );
                    self.cursor.advance(); // synthetic `]`
                    // `ensureCloseParen(token, begin)` (:1962) — same list-open
                    // jump target as the `[`/`{` arms.
                    end = self.ensure_close_paren(begin);
                    break;
                }
                _ => {}
            }
            parameter_count += 1;
            self.parse_formal_parameter(FormalParameterKind::RequiredPositional, kind);
            match self.peek_kind() {
                TokenKind::Comma => {
                    if kind == MemberKind::PrimaryConstructor {
                        let span = self.peek_span();
                        if let Some(state) = self.rep_field_check.as_mut()
                            && state.count == 1
                            && state.comma_after_first.is_none()
                        {
                            state.comma_after_first = Some(span);
                        }
                    }
                    self.cursor.advance();
                }
                TokenKind::CloseParen => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                _ => {
                    // Recovery: id-id suggests a missing comma (reported,
                    // tokens kept); everything else routes through
                    // `ensure_close_paren` — the official tail's single
                    // recovery exit (:1975-1985), whose `moveSynthetic`
                    // adoption reports the unmatched-group row at the
                    // relocation point and latches the pair for the parse-end
                    // pass.
                    end = self.recover_parameter_group_end(begin);
                    break;
                }
            }
        }
        self.last_parameter_count = parameter_count;
        self.listener
            .end_formal_parameters(parameter_count, begin, end, kind);
        self.leave_type_depth();
        end
    }

    /// The `parseFormalParametersRest` tail recovery: `id id` reports a
    /// missing comma and continues, everything else funnels into
    /// [`Self::ensure_close_paren`] — the official tail has no separate
    /// silent-adoption arm (`parser_impl.dart:1975-1985`); a scanner-synthetic
    /// closer adopts through `ensureCloseParen`'s `moveSynthetic`, which
    /// reports the unmatched-group row at the relocation point (the analyzer
    /// translation reads `begin.endGroup.charOffset` post-parse; the scanner
    /// itself emits no row — the pair list on the token table is the only
    /// scan-time record) and latches the begin so the parse-end pass skips
    /// it. A pre-#1485 silent relocation here relied on the scanner's own
    /// row, which no longer exists.
    fn recover_parameter_group_end(&mut self, begin: TokenSpan) -> TokenSpan {
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
            // The `parameterCount == 0` arm reports the pseudo-shared
            // `missingIdentifier` (fe91 codes :577-590, measured f14 —
            // "Expected an identifier." at this `]`).
            self.report(msg::missing_identifier(), at);
            // Queue the zero-length name and leave it current: official
            // `rewriteAndRecover` (:5112) reports on the old next (this `]`)
            // and splices the synthetic in front of it, then
            // `parseFormalParameter` (:2459) starts at the synthetic and
            // consumes it silently as the formal's name. Advancing past it
            // here would feed the `]` to the name step instead
            // (`missing_identifier` twice and a derailed list) — same shape
            // as the `[]`-token arm above.
            self.cursor
                .insert_synthetic(TokenKind::Identifier, at.range.offset);
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
            // The `parameterCount == 0` arm reports the pseudo-shared
            // `missingIdentifier` (fe91 codes :577-590, measured f14 —
            // "Expected an identifier." at this `}`).
            self.report(msg::missing_identifier(), at);
            // Queue the zero-length name and leave it current: official
            // `rewriteAndRecover` (:5112) reports on the old next (this `}`)
            // and splices the synthetic in front of it, then
            // `parseFormalParameter` (:2527) starts at the synthetic and
            // consumes it silently as the formal's type-or-name. Advancing
            // past it here would feed the `}` to the name step instead
            // (`missing_identifier` twice and a derailed list) — same shape
            // as the positional arm above.
            self.cursor
                .insert_synthetic(TokenKind::Identifier, at.range.offset);
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

        // The caller's kind, untouched by the walk's optional-named upgrade —
        // the walk-end `required` gate tests it (modifier_context.dart:212-221).
        let original_parameter_kind = parameter_kind;
        let mut required: Option<TokenSpan> = None;
        let mut covariant: Option<TokenSpan> = None;
        let mut var_final_or_const: Option<TokenSpan> = None;
        // A repeat `required` was seen in the walk: the oracle then reports
        // `extraneous_modifier` on the FIRST `required` even in a named
        // group (`{required required x}` → extraneous@first +
        // duplicated@second, probed 3.13.1), where a lone `required` stays
        // silent.
        let mut duplicate_required = false;
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
                    if matches!(
                        member_kind,
                        MemberKind::ExtensionNonStaticMethod | MemberKind::ExtensionStaticMethod
                    ) =>
                {
                    // `reportExtraneousModifierInExtension`
                    // (modifier_context.dart:230, analyzer code
                    // invalid_use_of_covariant_in_extension).
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::invalid_use_of_covariant_in_extension("covariant"), t);
                }
                TokenKind::Keyword(Keyword::Covariant)
                    if !matches!(
                        member_kind,
                        MemberKind::StaticMethod
                            | MemberKind::TopLevelMethod
                            | MemberKind::ExtensionTypeNonStaticMethod
                            | MemberKind::ExtensionTypeStaticMethod
                    ) =>
                {
                    // Recorded silently; the walk-end gate reports the
                    // primary-constructor case (modifier_context.dart:238)
                    // once `var`/`final`/`const` is known.
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
                TokenKind::Keyword(Keyword::Var | Keyword::Final) if in_function_type => {
                    // Function-type formals take the official recovery route
                    // (`parseFormalParameterModifiers`): the modifier is
                    // consumed and recorded — it flows back into
                    // `parseFormalParameter` as `varFinalOrConst` — and the
                    // `GeneralizedFunctionType` gate below reports
                    // `function_typed_parameter_var` in place of the return
                    // type (modifier_context.dart:278-286). The var/final
                    // walk-end gate adds `extraneous_modifier` (probed
                    // 3.13.1: `typedef F = void Function(final int inner);`
                    // → the pair on the `final`, tree intact).
                    if var_final_or_const.is_some() {
                        let t = self.peek_span();
                        self.report(msg::duplicated_modifier(self.cursor.lexeme(t.id)), t);
                    } else {
                        var_final_or_const = Some(self.peek_span());
                    }
                    self.cursor.advance();
                }
                TokenKind::Keyword(Keyword::Const) if var_final_or_const.is_none() => {
                    // `const` routes to the recovery walk's `constToken`
                    // branch in both flavors: `extraneous_modifier` only —
                    // the `function_typed_parameter_var` arm is the `else
                    // if` of the const test (modifier_context.dart:270-286),
                    // and the recorded token is never a `const`, so the
                    // walk-end VAR/FINAL split stays silent for it.
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::extraneous_modifier("const"), t);
                }
                TokenKind::Keyword(Keyword::Required) if required.is_some() => {
                    // `_parseRequired` recovery (modifier_context.dart:596).
                    let t = self.peek_span();
                    self.cursor.advance();
                    self.report(msg::duplicated_modifier("required"), t);
                    duplicate_required = true;
                }
                TokenKind::Keyword(Keyword::Required) => {
                    // Recorded silently; the walk-end gate reports it once
                    // (modifier_context.dart:212-221).
                    let t = self.peek_span();
                    self.cursor.advance();
                    required.get_or_insert(t);
                }
                _ => break,
            }
        }
        // Walk-end `required` gate (modifier_context.dart:212-221): one
        // `extraneous_modifier` at the first `required` when the formal's
        // kind never named it; repeats already reported `duplicated_modifier`
        // inside the walk. Primary constructors tolerate it on named formals.
        // A repeated `required` reports at the first one even in a named
        // group (probed 3.13.1: `{required required x}` → extraneous@first +
        // duplicated@second; a lone `required` stays silent).
        if let Some(t) = required {
            let kind_named = matches!(
                original_parameter_kind,
                FormalParameterKind::OptionalNamed | FormalParameterKind::RequiredNamed
            );
            let gate = if member_kind == MemberKind::PrimaryConstructor {
                !kind_named
            } else {
                original_parameter_kind != FormalParameterKind::OptionalNamed || duplicate_required
            };
            if gate {
                self.report(msg::extraneous_modifier("required"), t);
            }
        }
        if member_kind == MemberKind::GeneralizedFunctionType
            && let Some(t) = var_final_or_const
        {
            self.report(msg::function_typed_parameter_var(), t);
        }
        // `invalidCovariantModifierInPrimaryConstructor`
        // (modifier_context.dart:238-246): with the feature on, `covariant`
        // in a primary-constructor formal is only legal alongside `var`
        // (oracle 3.13.1, `class P(covariant int x) {}` → the face on the
        // `covariant` token).
        if member_kind == MemberKind::PrimaryConstructor
            && let Some(t) = covariant
            && self.features.primary_constructors_enabled
            && var_final_or_const.is_none_or(|v| self.cursor.lexeme(v.id) != "var")
        {
            self.report(msg::invalid_covariant_modifier_in_primary_constructor(), t);
        }
        self.listener.begin_formal_parameter(
            start,
            member_kind,
            required,
            covariant,
            var_final_or_const,
        );

        // Type (required in function types, optional otherwise); the formal
        // flavor lets `TYPE <reserved word>` read as type + keyword name
        // (official acceptKeywordForSimpleType, parser_impl.dart:2158).
        let type_info = self.compute_type_formal(in_function_type);

        // Official :2160 — the type region is consumed before the
        // this/super walk. `looksLikeName` accepts `this`/`super` in the
        // name position, so a written type commits even for `String this.x`
        // and the receiver walk must classify the token AFTER the type (the
        // pre-type walk never fired behind a written type and left `this`
        // for the name parse: `expected_identifier_but_got_keyword` +
        // `expected_token` where the oracle parses clean). The raw skip is
        // rewind-clean: the event-emitting type parse below re-walks from
        // `before_type`.
        let before_type = self.cursor.mark();
        type_info.skip_type_raw(self);

        let mut this_keyword: Option<TokenSpan> = None;
        let mut super_keyword: Option<TokenSpan> = None;
        let mut period_after_this_or_super: Option<TokenSpan> = None;
        let mut name_context = IdentifierContext::FormalParameterDeclaration;

        // The this/super walk (official :2177-2211), now reading the token
        // after the consumed type. The event-emitting type parse below
        // re-walks from `before_type`, so when the walk consumed a head the
        // parse arms step over the consumed `this`/`super` + period again
        // (the missing-`.` arm's synthetic period re-mints silently — its
        // report fired exactly once in the walk).
        let before_this = self.cursor.mark();
        let mut period_synthetic = false;
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
                    if self.external_ctor_field_formals {
                        self.report(msg::external_constructor_with_field_initializers(), keyword);
                    }
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
                period_synthetic = true;
                name_context = IdentifierContext::FieldInitializer;
                if is_this {
                    this_keyword = Some(keyword);
                    if self.external_ctor_field_formals {
                        self.report(msg::external_constructor_with_field_initializers(), keyword);
                    }
                } else {
                    super_keyword = Some(keyword);
                }
            }
        }

        // Declaring-parameter faces (official :2212-2226, run right after the
        // walk): a primary-constructor formal that carries `var`/`final` AND
        // a `this.`/`super.` receiver reports at the receiver keyword —
        // `initializingDeclaringParameter` for `this`, else
        // `superInitializingDeclaringParameter` for `super`. The official
        // gate is `varFinalOrConst != null && !isA(CONST)`: a `const` formal
        // never carries a declaring code and keeps the modifier walk's
        // `extraneous_modifier` as its only parser row. Here `const` is
        // reported in the walk without being recorded, so `is_some()` is
        // exactly the var/final test (probed 3.13.1, corpus default
        // ^3.13.0: `class C(var this.str);` → the declaring row at `this`;
        // `class C(const super.str);` → `extraneous_modifier` at `const`
        // only).
        if member_kind == MemberKind::PrimaryConstructor && var_final_or_const.is_some() {
            if let Some(kw) = this_keyword {
                self.report(msg::initializing_declaring_parameter(), kw);
            } else if let Some(kw) = super_keyword {
                self.report(msg::super_initializing_declaring_parameter(), kw);
            }
        }

        // Field formals are only legal in constructor formals; anywhere else
        // the verifier reports `field_initializer_outside_constructor` at the
        // `this` keyword (error_verifier.dart:5595; primary constructors are
        // exempt via `PrimaryConstructorDeclaration`).
        if let Some(kw) = this_keyword
            && member_kind != crate::listener::MemberKind::PrimaryConstructor
            && !self.in_constructor_formals
        {
            self.report(msg::field_initializer_outside_constructor(), kw);
        }

        // Inline function-typed parameter detection (official :2221-2264):
        // the token after the type is the tentative name; its `<...>(` or
        // direct `(` follower makes this an inline function-typed parameter
        // (a committed type never reaches the bare-`(` arm, so record-type
        // parameters like `((Enum, Enum) r)` parse as type + name). The type
        // region was already consumed above, so the probe reads the
        // post-type token directly (a `this` receiver formal never probes —
        // the walk owns it; a `super` receiver probes its name exactly as
        // the post-walk probe did).
        let mut before_inline_function_type: Option<TokenSpan> = None;
        let mut inline_name: Option<TokenSpan> = None;
        let mut type_param = TypeParamOrArg::None;
        let before_probe = self.cursor.mark();
        if this_keyword.is_none() {
            let tentative_name = is_identifier(self.peek_kind());
            let mut shape_matched = false;
            if tentative_name {
                let follower = self.cursor.look_kind(1);
                let name = self.peek_span();
                if follower == TokenKind::Lt {
                    self.cursor.advance();
                    type_param = self.compute_type_param_or_arg_on_lt(false);
                    // The shape follower is read after the group (official
                    // `closer.next!`), computed from the `<` offset so a
                    // raw-compound closer (`>>`/`>=`) reads its split
                    // remainder.
                    if self.type_arg_follower(&type_param, self.cursor.real_pos())
                        == TokenKind::OpenParen
                    {
                        shape_matched = true;
                    }
                } else if follower == TokenKind::OpenParen {
                    shape_matched = true;
                }
                if shape_matched {
                    if let Some(t) = var_final_or_const
                        && member_kind != MemberKind::PrimaryConstructor
                    {
                        // Official :2239-2247 — the shape-matched probe
                        // reports `function_typed_parameter_var` on the
                        // modifier; the `extraneous_modifier` half of the
                        // probed pair (`void g(var g()) {}` → both rows on
                        // the keyword) comes from the var/final walk-end
                        // gate below.
                        self.report(msg::function_typed_parameter_var(), t);
                    }
                    inline_name = Some(name);
                    before_inline_function_type = Some(name);
                }
            } else if self.peek_kind() == TokenKind::OpenParen
                && matches!(type_info, TypeInfo::NoType)
            {
                if let Some(t) = var_final_or_const
                    && member_kind != MemberKind::PrimaryConstructor
                {
                    // Same probed pair as the type-led shape above; the
                    // walk-end gate owns the `extraneous_modifier` half.
                    self.report(msg::function_typed_parameter_var(), t);
                }
                before_inline_function_type = Some(self.peek_span());
            }
            // The unified parse below re-walks from the type start (official
            // `parseType(beforeType)`). An untyped walked formal re-walks
            // nothing — its post-walk position (the name) stands, exactly
            // like the pre-reorder probe's reset target.
            if !matches!(type_info, TypeInfo::NoType) {
                self.cursor.reset(before_type);
            } else {
                self.cursor.reset(before_probe);
            }
        }

        let mut var_or_final = None;
        if let Some(t) = var_final_or_const {
            // Official walk-end gate (parser_impl.dart:2271-2285 var arm,
            // :2299-2307 final arm): `extraneous_modifier` when the
            // primary-constructors feature is on and this is not a
            // primary-constructor formal — the settled 3.13+ posture that
            // rejects `final`/`var` on ordinary formals (`void f(final int
            // x)` → the row on the keyword, probed 3.13.1). `const` is
            // never recorded (reported in the walk), so the var/final test
            // is exactly the official VAR/FINAL arm split.
            let is_var = self.cursor.lexeme(t.id) == "var";
            let is_final = self.cursor.lexeme(t.id) == "final";
            if is_var || is_final {
                var_or_final = Some(t);
                // Catch formals never surface the row (probed 3.13.1:
                // `catch (final e)` and `catch (e, final t)` → the
                // `catch_syntax` family only, no `extraneous_modifier` —
                // the official catch-shape scan consumes the keyword as the
                // invalid name before the group re-parse reaches the gate).
                if self.features.primary_constructors_enabled
                    && member_kind != MemberKind::PrimaryConstructor
                    && member_kind != MemberKind::Catch
                {
                    let lexeme = self.cursor.lexeme(t.id).to_owned();
                    self.report(msg::extraneous_modifier(&lexeme), t);
                }
                // `varAndType` for `var` + a type in ordinary formal
                // positions (parser_impl.dart:2286-2298). Primary-constructor
                // declaring parameters legitimately carry `var` + a type and
                // stay silent: the official gate keys on
                // `memberKind != MemberKind.PrimaryConstructor`, and the
                // front_end's feature-off `reportExperimentNotEnabled` arm
                // (:2290-2296) never fires on the analyzer — probed 3.13.1
                // at language version 3.12, `class C(var int a) {}` yields
                // zero rows. The `final` arm (:2299-2307) has no type gate.
                if is_var
                    && !matches!(type_info, TypeInfo::NoType)
                    && member_kind != MemberKind::PrimaryConstructor
                {
                    self.report(msg::var_and_type(), t);
                }
            }
        }

        if let Some(before) = before_inline_function_type {
            // Official :2294-2331: re-walk the return type, then the
            // name-relative type variables, then the parameter list.
            self.listener.begin_function_typed_formal_parameter(before);
            type_info.parse_type(self);
            if self.peek_kind() != TokenKind::Lt && self.peek_kind() != TokenKind::OpenParen {
                // Step over the name onto its `<...>`/`(` suffix.
                self.cursor.advance();
            }
            if !matches!(type_param, TypeParamOrArg::None) {
                type_param.parse_variables(self, false);
            }
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
            // A consumed receiver head sits between the type and the name.
            // With a written type the re-walk stopped on the already-consumed
            // `this`/`super`, so step over the head again after the type
            // parse (official order — skipType (:2160), the walk
            // (:2177-2211), then the event-emitting `parseType(beforeType)`
            // (:2332)). The missing-`.` arm's synthetic period re-mints
            // silently — its report fired exactly once in the walk, and the
            // payload span is re-captured from the fresh synthetic. An
            // untyped walked formal re-walks nothing and is already on the
            // name.
            let stepped_receiver =
                period_after_this_or_super.is_some() && !matches!(type_info, TypeInfo::NoType);
            if stepped_receiver {
                self.cursor.reset(before_type);
            }
            type_info.parse_type(self);
            if stepped_receiver {
                self.cursor.advance(); // `this`/`super`
                if period_synthetic {
                    let at = self.peek_span();
                    let id = self
                        .cursor
                        .insert_synthetic(TokenKind::Period, at.range.offset);
                    self.cursor.advance();
                    period_after_this_or_super = Some(self.span_of(id));
                } else {
                    self.cursor.advance(); // the real `.`
                }
            }
        }

        // Name. A TRUE-JUNK name slot behind a real type takes the official
        // `handleNoName` posture: 3.13.1 `parseFormalParameter` consumes a
        // typed parameter's name only `if (next.isIdentifier)`
        // (parser_impl.dart:2229) with no recovery report — the junk stays
        // current for the parameter list's tail, whose `ensureCloseParen`
        // carries the single row anchored on the junk (oracle probe
        // `void f(int 5) {}`: exactly one `expected_token` `Expected to
        // find ')'.` at the `5`; no `missing_identifier`). Every other
        // non-identifier face keeps the #184 template route (oracle
        // probes): a reserved word reports
        // `expected_identifier_but_got_keyword` and is consumed even
        // behind a type (`void t1(int assert)`); a no-type junk token
        // (`void f(5) {}`) reports `missing_identifier` at the junk;
        // separators keep the insert-without-consuming branch. Named
        // parameters in function types keep the 3.15 arm this crate
        // already pinned, and `this.`/`super.` formals (`FieldInitializer`
        // context) always ensure a name (parser_impl.dart:2371).
        let name = if let Some(n) = inline_name {
            n
        } else if !(before_inline_function_type.is_some()
            || name_context != IdentifierContext::FormalParameterDeclaration
            || matches!(type_info, TypeInfo::NoType)
            || is_identifier(self.peek_kind())
            || matches!(
                self.peek_kind(),
                TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Reserved
            )
            || (in_function_type
                && matches!(
                    parameter_kind,
                    FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
                )))
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
                // Official separator split (:2386-2395 at the r5 anchor;
                // 3.15 folded the two `:` faces into
                // positionalParameterWithEquals, the 3.13 oracle keeps the
                // dedicated code the corpus pins): `:` on an optional
                // positional wants `=`; on a required positional BOTH
                // separators route to `namedParameterOutsideGroup` (`=`'s
                // dedicated `requiredParameterWithDefault` is pseudo-shared
                // to the same code, fe91 codes_generated.dart:2742-2746;
                // measured f15/b05); function-typed contexts forbid defaults
                // outright.
                if parameter_kind == FormalParameterKind::RequiredPositional {
                    self.report(msg::named_parameter_outside_group(), equals);
                } else if parameter_kind == FormalParameterKind::OptionalPositional
                    && separator == TokenKind::Colon
                {
                    self.report(msg::wrong_separator_for_positional_parameter(), equals);
                } else if in_function_type
                    || member_kind == MemberKind::FunctionTypeAlias
                    || member_kind == MemberKind::FunctionTypedParameter
                {
                    self.report(msg::default_value_in_function_type(), equals);
                }
            }
            _ => {
                let next = self.peek_span();
                self.listener.handle_formal_parameter_without_value(next);
            }
        }
        // Representation-field shape (extension type primary constructor
        // only; nested function-typed formals use a different member kind).
        if let Some(state) = self.rep_field_check.as_mut()
            && member_kind == MemberKind::PrimaryConstructor
        {
            if state.count == 0 {
                state.modifier = var_final_or_const;
                state.this_or_super = this_keyword.is_some() || super_keyword.is_some();
                state.fn_typed = before_inline_function_type.is_some();
                state.has_type = !matches!(type_info, TypeInfo::NoType);
            }
            state.count += 1;
        }
        self.listener.end_formal_parameter(
            var_or_final,
            covariant,
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
