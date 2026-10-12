//! Parse-side type consumption (official `parseType` family from
//! `type_info_impl.dart`): the `ensure*`/`parse*` shapes that emit events
//! while consuming, plus the official record-type grammar
//! (`parseRecordType`, `parser_impl.dart:1747-1907`).

use dartq_scanner::{Keyword, TokenKind};

use crate::listener::{IdentifierContext, MemberKind, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::recovery::is_identifier;
use crate::token_id::{TokenId, TokenSpan};
use crate::type_args::RecordProbe;
use crate::type_args::TypeParamOrArg;
use crate::type_args::simple1_closer;
use crate::types::{ComplexType, TypeInfo};

// ---------------------------------------------------------------------------
// parse-side shapes (type_info_impl.dart parseType family)
// ---------------------------------------------------------------------------

impl TypeInfo {
    /// Last token of the type (`skipType`, `type_info.dart:74`). Cursor
    /// convention: the cursor sits ON the type's first token, so all offsets
    /// are relative to it.
    pub(crate) fn skip_type(&self, parser: &Parser<'_, '_, impl ParserListener>) -> TokenId {
        let start = parser.peek_id();
        match self {
            TypeInfo::NoType => start.saturating_sub(1),
            // VoidType.skipType (type_info_impl.dart:469-479): `void`
            // followed by type arguments skips the whole group — the
            // official `computeTypeParamOrArg != noTypeParamOrArg` probe
            // gates it and `typeParam.skip` lands on the group's closer
            // (a compound closer names the official split leading `>`,
            // #135). Probed read-only like `skip_type_len`'s void arm;
            // exact for valid input.
            TypeInfo::Void => {
                if parser.cursor.look_kind(1) == TokenKind::Lt {
                    let lt = parser.cursor.real_pos() + 1;
                    if simple1_closer(parser, lt).is_some() {
                        // `<` id closer: the `<`, the argument, then the
                        // closer (SimpleTypeArgument1.skipEndGroup).
                        return start + 3;
                    }
                    if let Some(len) = parser.gt_group_len(lt) {
                        // `len` counts table tokens and the table is
                        // u32-positioned, so the cast cannot truncate.
                        #[expect(clippy::cast_possible_truncation, reason = "see above")]
                        return start + len as u32;
                    }
                }
                start
            }
            TypeInfo::Simple => start,
            TypeInfo::SimpleNullable => start + 1,
            TypeInfo::Prefixed => start + 2,
            // `id<id>`: `<`, the argument, then the closer (official
            // `SimpleTypeWith1Argument.skipType` + `SimpleTypeArgument1.skip`,
            // type_info_impl.dart:302/:1184); the nullable variant adds the
            // `?` (:239). A compound closer still names the combined token —
            // the position of the official split leading `>` (#135).
            TypeInfo::SimpleWith1Arg { nullable: false } => start + 3,
            TypeInfo::SimpleWith1Arg { nullable: true } => start + 4,
            TypeInfo::Complex(c) => c.end,
        }
    }

    /// Raw skip: advances the cursor past the type without emitting events
    /// (the stand-in for the official suppressed-parse recovery swaps).
    pub(crate) fn skip_type_raw(&self, parser: &mut Parser<'_, '_, impl ParserListener>) {
        let end = self.skip_type(parser);
        let mut guard = 0usize;
        while parser.cursor.peek_id() < end && !parser.cursor.at_eof() && guard < parser.table.len()
        {
            parser.cursor.advance();
            guard += 1;
        }
        if !matches!(self, TypeInfo::NoType)
            && parser.cursor.peek_id() == end
            && !parser.cursor.at_eof()
        {
            parser.cursor.advance();
        }
    }

    /// Official `parseType`: optional type or void; emits events.
    pub(crate) fn parse_type(&self, parser: &mut Parser<'_, '_, impl ParserListener>) -> TokenSpan {
        match self {
            TypeInfo::NoType => {
                let span = parser.peek_span();
                parser.listener.handle_no_type(span);
                span
            }
            TypeInfo::Void => parse_void(parser),
            TypeInfo::Simple => parse_simple(parser, false),
            TypeInfo::SimpleNullable => parse_simple(parser, true),
            TypeInfo::Prefixed => parse_prefixed(parser),
            TypeInfo::SimpleWith1Arg { nullable } => parse_simple_with_1_arg(parser, *nullable),
            TypeInfo::Complex(c) => c.parse_type(parser),
        }
    }

    /// Official `parseTypeNotVoid`.
    pub(crate) fn parse_type_not_void(
        &self,
        parser: &mut Parser<'_, '_, impl ParserListener>,
    ) -> TokenSpan {
        match self {
            TypeInfo::Void => {
                // Report and parse `void` as a type name
                // (VoidType.ensureTypeNotVoid, type_info_impl.dart:431).
                let void_tok = parser.peek_span();
                parser.report(msg::invalid_void(), void_tok);
                parser.cursor.advance();
                let name = parser.peek_span();
                parser
                    .listener
                    .handle_identifier(name, IdentifierContext::TypeReference);
                parser.listener.handle_type(name, None);
                name
            }
            other => other.parse_type(parser),
        }
    }

    /// Official `ensureTypeNotVoid`.
    pub(crate) fn ensure_type_not_void(
        &self,
        parser: &mut Parser<'_, '_, impl ParserListener>,
    ) -> TokenSpan {
        match self {
            TypeInfo::NoType => synthetic_type_reference(parser),
            TypeInfo::Void => self.parse_type_not_void(parser),
            other => other.parse_type(parser),
        }
    }

    /// Official `ensureTypeOrVoid`.
    pub(crate) fn ensure_type_or_void(
        &self,
        parser: &mut Parser<'_, '_, impl ParserListener>,
    ) -> TokenSpan {
        match self {
            TypeInfo::NoType => synthetic_type_reference(parser),
            other => other.parse_type(parser),
        }
    }
}

/// Reports `Expected a type name.` and emits a synthetic simple type
/// (the `NoType` ensure path, type_info_impl.dart:117-121).
fn synthetic_type_reference(parser: &mut Parser<'_, '_, impl ParserListener>) -> TokenSpan {
    let at = parser.peek_span();
    parser.report(msg::expected_type_name(), at);
    let synthetic = parser.synthetic(TokenKind::Identifier, at.range.offset);
    parser
        .listener
        .handle_identifier(synthetic, IdentifierContext::TypeReference);
    parser.listener.handle_type(synthetic, None);
    synthetic
}

/// `VoidType.parseType` (`type_info_impl.dart:446`).
fn parse_void(parser: &mut Parser<'_, '_, impl ParserListener>) -> TokenSpan {
    let void_span = {
        let id = parser.cursor.advance();
        parser.span_of(id)
    };
    // Recovery: `void<...>` is reported but consumed. Official anchor is
    // `token.next` — the `<` after `void` (VoidType.parseType,
    // type_info_impl.dart:446).
    if parser.peek_kind() == TokenKind::Lt {
        let type_param = parser.compute_type_param_or_arg_on_lt(false);
        if !matches!(type_param, TypeParamOrArg::None) {
            parser.report(msg::void_with_type_arguments(), parser.peek_span());
            type_param.parse_arguments(parser);
            parser
                .listener
                .handle_void_keyword_with_type_arguments(void_span);
            return parser.peek_span();
        }
    }
    parser.listener.handle_void_keyword(void_span);
    void_span
}

/// `SimpleType.parseType` / `SimpleNullableType.parseType`.
fn parse_simple(parser: &mut Parser<'_, '_, impl ParserListener>, nullable: bool) -> TokenSpan {
    let name = parser.peek_span();
    parser.cursor.advance();
    parser
        .listener
        .handle_identifier(name, IdentifierContext::TypeReference);
    if nullable {
        let q = {
            let id = parser.cursor.advance();
            parser.span_of(id)
        };
        parser.listener.handle_type(name, Some(q));
        q
    } else {
        parser.listener.handle_type(name, None);
        name
    }
}

/// `PrefixedType.parseType` (`type_info_impl.dart:181`).
fn parse_prefixed(parser: &mut Parser<'_, '_, impl ParserListener>) -> TokenSpan {
    let start = parser.peek_span();
    parser.cursor.advance();
    parser
        .listener
        .handle_identifier(start, IdentifierContext::PrefixedTypeReference);
    let period = {
        let id = parser.cursor.advance();
        parser.span_of(id)
    };
    let cont = parser.peek_span();
    parser.cursor.advance();
    parser
        .listener
        .handle_identifier(cont, IdentifierContext::TypeReferenceContinuation);
    parser.listener.handle_qualified(period);
    parser.listener.handle_no_type_arguments(parser.peek_span());
    parser.listener.handle_type(start, None);
    cont
}

/// `SimpleTypeWith1Argument.parseType` / `SimpleNullableTypeWith1Argument`
/// (`type_info_impl.dart:288/231)`: `id < id closer [?]`.
fn parse_simple_with_1_arg(
    parser: &mut Parser<'_, '_, impl ParserListener>,
    nullable: bool,
) -> TokenSpan {
    let start = parser.peek_span();
    parser.cursor.advance();
    parser
        .listener
        .handle_identifier(start, IdentifierContext::TypeReference);
    let begin = parser.peek_span(); // `<`
    parser.cursor.advance();
    parser.listener.begin_type_arguments(begin);
    let arg = parser.peek_span();
    parser.cursor.advance();
    parser
        .listener
        .handle_identifier(arg, IdentifierContext::TypeReference);
    parser.listener.handle_type(arg, None);
    debug_assert!(parser.cursor.peek_is_closer());
    let closer = {
        let id = parser.cursor.consume_closer();
        parser.span_of(id)
    };
    parser.listener.end_type_arguments(1, begin, closer);
    if nullable {
        let q = {
            let id = parser.cursor.advance();
            parser.span_of(id)
        };
        parser.listener.handle_type(start, Some(q));
        q
    } else {
        parser.listener.handle_type(start, None);
        start
    }
}

impl ComplexType {
    /// Official `ComplexTypeInfo.parseType` (`type_info_impl.dart:616`).
    pub(crate) fn parse_type(&self, parser: &mut Parser<'_, '_, impl ParserListener>) -> TokenSpan {
        let start = parser.span_of(self.start);
        let starter_count = self.type_variable_starters.len();

        // Pass 1: begin events + type variables, outermost `Function` first.
        let mut type_variable_ends: Vec<TokenSpan> = Vec::with_capacity(starter_count);
        for starter in self.type_variable_starters.iter().rev() {
            parser.listener.begin_function_type(start);
            let mark = parser.cursor.mark();
            parser.cursor.jump_to(*starter + 1);
            let type_param = parser.compute_type_param_or_arg_on_lt(false);
            let end = type_param.parse_variables(parser, false);
            type_variable_ends.push(end);
            parser.cursor.reset(mark);
        }

        // Return type (or record type, or absent).
        let mut end = if self.gft_has_return_type == Some(false) {
            let at = parser.peek_span();
            parser.listener.handle_no_type(at);
            at
        } else if self.is_record_type || self.gft_return_type_has_record_type {
            // `isQuestionPartOfType` (type_info_impl.dart:655): the trailing
            // `?` belongs to the record type when compute captured it
            // (`beforeQuestion != null`), otherwise it is the follower's.
            parser.parse_record_type(
                self.gft_return_type_has_record_type || self.before_question.is_some(),
            )
        } else {
            parse_complex_plain_type(self, parser)
        };

        // Pass 2: walk the `Function` keywords, innermost first.
        for i in 0..starter_count {
            debug_assert_eq!(parser.peek_kind(), TokenKind::Keyword(Keyword::Function));
            let function_token = parser.peek_span();
            parser.cursor.advance();
            if parser.peek_kind() == TokenKind::Lt {
                // Skip type parameters; they were parsed in pass 1.
                let type_end = type_variable_ends[starter_count - 1 - i];
                parser.skip_to_token(type_end.id);
                parser.cursor.advance();
            }
            end = parser.parse_formal_parameters_required_opt(MemberKind::GeneralizedFunctionType);
            let mut question = None;
            if parser.peek_kind() == TokenKind::Question
                && (i < starter_count - 1 || self.before_question.is_some())
            {
                let q = {
                    let id = parser.cursor.advance();
                    parser.span_of(id)
                };
                question = Some(q);
                end = q;
            }
            parser.listener.end_function_type(function_token, question);
        }

        end
    }

    /// `computeRest` (`type_info_impl.dart:1088)`: record `?` and `Function`
    /// suffixes starting the scan at `from`'s successor. Read-only.
    pub(crate) fn compute_rest(
        &mut self,
        parser: &Parser<'_, '_, impl ParserListener>,
        from: TokenId,
        required: bool,
    ) {
        // A raw compound closer at `from` (splitCloser,
        // type_info_impl.dart:1772): the group's own closer is the leading
        // `>` and the official successor is the split remainder — never a
        // `?` and never a `Function` intro (#168) — so the scan is a no-op.
        if self.end_raw_compound {
            return;
        }
        let mut offset = parser.offset_of(from);
        if parser.kind_at(offset + 1) == TokenKind::Question {
            self.before_question = Some(parser.real_id(offset));
            self.end = parser.real_id(offset + 1);
            offset += 1;
        }
        offset += 1;
        while parser.kind_at(offset) == TokenKind::Keyword(Keyword::Function) {
            let type_variable_start = parser.real_id(offset);
            // Official: `token = computeTypeParamOrArg(token, true).skip(token);
            // token = token.next!` — the probe lands ON the `(`, one step
            // past the `Function` or its `<...>` group.
            let mut probe = offset + 1;
            if parser.kind_at(probe) == TokenKind::Lt {
                match parser.cursor.table_matching_end(parser.real_id(probe)) {
                    Some(end) => probe = parser.offset_of(end) + 1,
                    None => break,
                }
            }
            if parser.kind_at(probe) != TokenKind::OpenParen {
                break; // Not a function type.
            }
            let Some(close) = parser.cursor.table_matching_end(parser.real_id(probe)) else {
                break; // Not a function type.
            };
            if !required {
                let mut follower = parser.offset_of(close) + 1;
                if parser.kind_at(follower) == TokenKind::Question {
                    follower += 1;
                }
                let follower_kind = parser.kind_at(follower);
                if !is_identifier(follower_kind)
                    && !matches!(
                        follower_kind,
                        TokenKind::Keyword(Keyword::This | Keyword::Super)
                    )
                {
                    break; // `Function` used as a declaration name.
                }
            }
            self.gft_has_return_type
                .get_or_insert(type_variable_start != self.start);
            self.type_variable_starters.push(type_variable_start);
            self.before_question = None;
            self.end = close;
            offset = parser.offset_of(close) + 1;
            if parser.kind_at(offset) == TokenKind::Question {
                self.before_question = Some(self.end);
                self.end = parser.real_id(offset);
                offset += 1;
            }
        }
        let _ = required;
    }

    /// `computeSimpleWithTypeArguments` (`type_info_impl.dart:1048`).
    pub(crate) fn compute_simple_with_type_args(
        &mut self,
        parser: &Parser<'_, '_, impl ParserListener>,
        required: bool,
    ) {
        self.end = self.type_args.skip(self.start);
        self.end_raw_compound = self.type_args.ends_on_raw_compound();
        self.compute_rest(parser, self.end, required);
    }

    /// `computePrefixedType` (`type_info_impl.dart:1068`).
    pub(crate) fn compute_prefixed_type(
        &mut self,
        parser: &Parser<'_, '_, impl ParserListener>,
        required: bool,
    ) {
        let mut offset = parser.offset_of(self.start);
        if parser.kind_at(offset) != TokenKind::Period {
            offset += 1;
        }
        // `offset` is at the `.`; continue over the continuation token when
        // keyword-or-identifier shaped.
        let cont = parser.kind_at(offset + 1);
        if is_identifier(cont) || matches!(cont, TokenKind::Keyword(_)) {
            offset += 1;
        }
        self.end = self.type_args.skip(parser.real_id(offset));
        self.end_raw_compound = self.type_args.ends_on_raw_compound();
        self.compute_rest(parser, self.end, required);
    }

    /// `computeRecordType` (`type_info_impl.dart:762)`: validate the
    /// parenthesized shape and record `end`/`?`. Cursor is before the `(`.
    pub(crate) fn compute_record_type(
        &mut self,
        parser: &Parser<'_, '_, impl ParserListener>,
        required: bool,
    ) {
        let open = parser.peek_id();
        let Some(end_group) = parser.cursor.table_matching_end(open) else {
            return;
        };
        self.recovered |= !RecordProbe {
            open: parser.offset_of(open),
        }
        .looks_like_record_type(parser, parser.offset_of(end_group));
        self.before_question = None;
        self.end = end_group;
        let after = parser.offset_of(end_group) + 1;
        if parser.kind_at(after) == TokenKind::Question {
            self.before_question = Some(self.end);
            self.end = parser.real_id(after);
        }
        self.is_record_type = true;
        let _ = required;
    }
}

/// The plain (non-record) type-reference branch of `ComplexTypeInfo.parseType`
/// (type_info_impl.dart:656-692).
fn parse_complex_plain_type(
    complex: &ComplexType,
    parser: &mut Parser<'_, '_, impl ParserListener>,
) -> TokenSpan {
    let name;
    if parser.peek_kind() == TokenKind::Keyword(Keyword::Void) {
        return parse_void(parser);
    }
    let is_prefixed = parser.peek_kind() == TokenKind::Period
        || (is_identifier(parser.peek_kind()) && parser.cursor.look_kind(1) == TokenKind::Period);
    if is_prefixed {
        name = parser.ensure_identifier(IdentifierContext::PrefixedTypeReference);
        let period = {
            let id = parser.cursor.advance();
            parser.span_of(id)
        };
        parser.ensure_identifier(IdentifierContext::TypeReferenceContinuation);
        parser.listener.handle_qualified(period);
    } else {
        name = parser.ensure_identifier(IdentifierContext::TypeReference);
    }
    // Consume the `?` only when it belongs to this type.
    let mut end = name;
    if !matches!(complex.type_args, TypeParamOrArg::None) {
        end = complex.type_args.parse_arguments(parser);
    }
    if parser.peek_kind() == TokenKind::Question
        && (!complex.type_variable_starters.is_empty() || complex.before_question.is_some())
    {
        let q = {
            let id = parser.cursor.advance();
            parser.span_of(id)
        };
        end = q;
        parser.listener.handle_type(name, Some(q));
    } else {
        parser.listener.handle_type(name, None);
    }
    end
}

// ---------------------------------------------------------------------------
// record types (parser_impl.dart:1747-1907)
// ---------------------------------------------------------------------------

impl<L: ParserListener> Parser<'_, '_, L> {
    /// Official `parseRecordType`. The cursor sits before the `(`.
    /// `recordType ::= '(' recordTypeFields ','? recordTypeNamedFields? ')'`.
    ///
    /// Depth-guarded: record fields recurse into `compute_type` and back into
    /// nested record types, so `((((...` is unbounded recursion without the
    /// guard (official `recoverFromStackOverflow`).
    pub(crate) fn parse_record_type(&mut self, is_question_part_of_type: bool) -> TokenSpan {
        if !self.enter_depth() {
            return self.peek_span();
        }
        let start = self.peek_span(); // `(`
        self.listener.begin_record_type(start);
        self.cursor.advance();

        let mut parameter_count = 0usize;
        let mut has_named_fields = false;
        let mut saw_comma = false;
        let mut illegal_trailing_comma: Option<TokenSpan> = None;
        let end;
        loop {
            let kind = self.peek_kind();
            if kind == TokenKind::CloseParen {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if parameter_count == 0
                && kind == TokenKind::Comma
                && self.cursor.look_kind(1) == TokenKind::CloseParen
            {
                illegal_trailing_comma = Some(self.peek_span());
                self.cursor.advance();
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            parameter_count += 1;
            if kind == TokenKind::OpenCurly {
                has_named_fields = true;
                self.parse_record_type_named_fields();
                // `ensureCloseParen` consumes the `)` in every arm (real,
                // relocated-synthetic, skipped, or inserted) and returns its
                // span with the cursor already past it — the official named
                // arm consumes `token.next!` directly
                // (parser_impl.dart:1747-1907). Re-peeking after the consume
                // minted `end` from the FOLLOWING token and the trailing
                // advance swallowed it (`({int a}) t;` lost the `t`).
                end = self.ensure_close_paren(start);
                break;
            }
            self.parse_record_type_field(true);
            if self.peek_kind() != TokenKind::Comma {
                if self.peek_kind() == TokenKind::CloseParen {
                    end = self.peek_span();
                    self.cursor.advance();
                } else if let Some(close) = self.current_matching_end_for(start) {
                    // Recovery: jump to the group's real or scanner-synthetic
                    // closer; the scanner already reported synthetic ones.
                    self.ensure_close_paren(start);
                    end = self.span_of(close);
                } else {
                    self.ensure_close_paren(start);
                    end = self.peek_span();
                }
                break;
            }
            saw_comma = true;
            self.cursor.advance();
        }

        if let Some(comma) = illegal_trailing_comma {
            self.report(msg::empty_record_type_with_comma(), comma);
        } else if parameter_count == 1 && !has_named_fields && !saw_comma {
            self.report(msg::record_type_one_positional_no_trailing_comma(), end);
        }

        // Only consume the `?` when it belongs to the type.
        let mut question = None;
        if self.peek_kind() == TokenKind::Question && is_question_part_of_type {
            let q = {
                let id = self.cursor.advance();
                self.span_of(id)
            };
            question = Some(q);
        }
        self.listener
            .end_record_type(start, question, parameter_count, has_named_fields);
        self.leave_depth();
        question.unwrap_or(end)
    }

    pub(crate) fn current_matching_end_for(&self, open: TokenSpan) -> Option<TokenId> {
        self.cursor.table_matching_end(open.id)
    }

    /// Official `parseRecordTypeField`
    /// (`recordTypeField ::= metadata type identifier?`).
    fn parse_record_type_field(&mut self, identifier_is_optional: bool) {
        let first = self.peek_span();
        self.listener.begin_record_type_entry(first);
        self.parse_metadata_star();
        let type_info = self.compute_type(true, false);
        let mut last = type_info.ensure_type_or_void(self);
        if self.peek_is_identifier() || !identifier_is_optional {
            last = self.ensure_identifier(IdentifierContext::RecordFieldDeclaration);
        } else {
            let next = self.look_span_at(1);
            self.listener.handle_no_name(next);
        }
        self.listener.end_record_type_entry(last);
    }

    /// Official `parseRecordTypeNamedFields`
    /// (`recordTypeNamedField ::= metadata type identifier`).
    fn parse_record_type_named_fields(&mut self) {
        let begin = self.peek_span(); // `{`
        self.cursor.advance();
        self.listener.begin_record_type_named_fields(begin);
        let mut parameter_count = 0usize;
        loop {
            if self.peek_kind() == TokenKind::CloseCurly {
                break;
            }
            self.parse_record_type_field(false);
            parameter_count += 1;
            if self.peek_kind() != TokenKind::Comma {
                if self.peek_kind() != TokenKind::CloseCurly {
                    // Recovery: the scanner guarantees the closing brace.
                    self.report(msg::expected_but_got("}"), self.peek_span());
                    if let Some(end) = self.current_matching_end_for(begin) {
                        self.skip_to_token(end);
                    }
                }
                break;
            }
            self.cursor.advance();
        }
        if parameter_count == 0 {
            self.report(msg::empty_record_type_named_fields_list(), self.peek_span());
        }
        let close = {
            let id = self.cursor.advance();
            self.span_of(id)
        }; // `}`
        let _ = close;
        self.listener
            .end_record_type_named_fields(parameter_count, begin);
    }
}
