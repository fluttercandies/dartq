//! Type annotation grammar, ported from `type_info.dart` + `type_info_impl.dart`
//! (Dart 3.15).
//!
//! The official design is *lazy*: `computeType` analyzes the lookahead
//! without consuming and returns one of eight `TypeInfo` shapes; a later
//! `parseType`/`ensureType*`/`skipType` call consumes and emits events. The
//! same shapes live here as [`TypeInfo`]; captured token ids make
//! compute-time knowledge directly usable at parse time.
//!
//! Generalized function types parse their type parameters through a cursor
//! leap: the cursor jumps forward to each `Function` token's `<...>` group
//! during the begin pass and is restored, and the walk pass skips the
//! already-parsed groups — exactly the official two passes over
//! `typeVariableStarters` in `ComplexTypeInfo.parseType`.
//!
//! Documented reductions (foundation grade, revisited with #18/#20): the
//! `ComplexTypeParamOrArgInfo` deep-recovery path (`parseUnexpectedEnd`
//! re-parsing garbage with a suppressed listener) is replaced by
//! report-once + jump to the scanner-paired `>`; unmatched `<` groups
//! during skip analysis yield "no type" instead of the official aggressive
//! in-declaration recovery.

pub use crate::type_args::TypeParamOrArg;
use dartq_scanner::{Keyword, TokenKind};

use crate::listener::ParserListener;
use crate::parser::Parser;
use crate::recovery::is_identifier;
use crate::token_id::TokenId;

/// The closer shape of an `id<id>` type argument (official
/// `SimpleTypeArgument1` / `+GtEq` / `+GtGt`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimpleCloser {
    Gt,
    GtEq,
    GtGt,
}

/// `TypeInfo` (`type_info.dart:22)`: the shape analysis result.
#[derive(Clone, Debug)]
pub enum TypeInfo {
    /// `NoType`: nothing that looks like a type.
    NoType,
    /// `void`.
    Void,
    /// `id`.
    Simple,
    /// `id?`.
    SimpleNullable,
    /// `id.id`.
    Prefixed,
    /// `id<id>` / `id<id>?` in the fast single-argument shape.
    SimpleWith1Arg { nullable: bool },
    /// Everything else (record types, function types, prefixed types with
    /// arguments, recovered shapes).
    Complex(ComplexType),
}

impl TypeInfo {
    /// Whether the shape's last token is a `?` (official
    /// `TypeInfo.isNullable`, `type_info.dart`).
    pub(crate) fn is_nullable(&self) -> bool {
        match self {
            TypeInfo::SimpleNullable | TypeInfo::SimpleWith1Arg { nullable: true } => true,
            _ => false,
        }
    }
}

/// `ComplexTypeInfo` (`type_info_impl.dart:526`). Ids are captured at compute
/// time; `end` is the last token of the type.
#[derive(Clone, Debug)]
pub struct ComplexType {
    pub(crate) start: TokenId,
    pub(crate) type_args: TypeParamOrArg,
    pub(crate) before_question: Option<TokenId>,
    pub(crate) end: TokenId,
    /// `end` names a raw compound closer (`>=`/`>>`/...) whose *leading*
    /// `>` closed the group (the official parser splits it there,
    /// `splitCloser`, `type_info_impl.dart:1772): the successor reads the
    /// split remainder, never a real token.
    pub(crate) end_raw_compound: bool,
    /// `Function` tokens seen during analysis, in source order.
    pub(crate) type_variable_starters: Vec<TokenId>,
    /// `Some(_)` for a generalized function type; `Some(false)` when there
    /// is no return type (`Function(...)` in type position).
    pub(crate) gft_has_return_type: Option<bool>,
    pub(crate) is_record_type: bool,
    pub(crate) gft_return_type_has_record_type: bool,
    pub(crate) recovered: bool,
}

// ---------------------------------------------------------------------------
// shape predicates (type_info.dart:128-151, type_info_impl.dart:487,1753)
// ---------------------------------------------------------------------------

/// `isGeneralizedFunctionType`: `Function` followed by `<` or `(`.
fn is_generalized_function_type(kind: TokenKind, next: TokenKind) -> bool {
    kind == TokenKind::Keyword(Keyword::Function)
        && matches!(next, TokenKind::Lt | TokenKind::OpenParen)
}

/// `isValidNonRecordTypeReference`.
pub(crate) fn is_valid_non_record_type_reference(kind: TokenKind, next: TokenKind) -> bool {
    match kind {
        TokenKind::Identifier => true,
        TokenKind::Keyword(kw) => {
            kw.style() == dartq_scanner::KeywordStyle::Pseudo
                || (kw.style() == dartq_scanner::KeywordStyle::BuiltIn && next == TokenKind::Period)
                || matches!(kw, Keyword::Dynamic | Keyword::Function | Keyword::Void)
        }
        _ => false,
    }
}

/// `isCloser` (`type_info_impl.dart:1753`).
pub(crate) fn is_closer(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::GtGt
            | TokenKind::GtGtGt
            | TokenKind::GtGtEq
            | TokenKind::GtGtGtEq
    )
}

/// A closer holding more than one `>` (`>=`, `>>`, ...). The official parser
/// splits these so the group's own closer is the leading `>` and the
/// remainder follows it (`splitCloser`, `type_info_impl.dart:1772`); at
/// compute time ours stays whole, so probes after such a token must read the
/// remainder instead of the real successor.
pub(crate) fn is_compound_closer(kind: TokenKind) -> bool {
    is_closer(kind) && kind != TokenKind::Gt
}

/// `isVariance` (`type_info_impl.dart:1746`).
pub(crate) fn is_variance(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Keyword(Keyword::In | Keyword::Inout | Keyword::Out)
    )
}

// ---------------------------------------------------------------------------
// Parser position/shape helpers shared by the grammar modules
// ---------------------------------------------------------------------------

impl<L: ParserListener> Parser<'_, '_, L> {
    /// Real-stream kind at an absolute table offset (EOF-pinned).
    #[inline]
    pub(crate) fn kind_at(&self, offset: usize) -> TokenKind {
        self.cursor.peek_kind_ahead_abs(offset)
    }

    #[inline]
    pub(crate) fn offset_of(&self, id: TokenId) -> usize {
        (id as usize).min(self.table.len() - 1)
    }

    /// `isGeneralizedFunctionType` at cursor-relative offset `n`.
    pub(crate) fn is_generalized_function_type_at(&self, n: usize) -> bool {
        is_generalized_function_type(
            self.cursor.peek_kind_ahead(n),
            self.cursor.peek_kind_ahead(n + 1),
        )
    }

    /// `isGeneralizedFunctionType` at an absolute id.
    pub(crate) fn is_generalized_function_type_at_id(&self, id: TokenId) -> bool {
        let o = self.offset_of(id);
        is_generalized_function_type(self.kind_at(o), self.kind_at(o + 1))
    }

    /// `isPossibleRecordType` at the cursor: `(` with a non-synthetic
    /// scanner-paired `)`.
    pub(crate) fn peek_is_possible_record_type(&self) -> bool {
        if self.peek_kind() != TokenKind::OpenParen {
            return false;
        }
        self.cursor
            .table_matching_end(self.peek_id())
            .is_some_and(|end| !self.cursor.resolve(end).1)
    }

    /// Matching close id for the open group at the cursor.
    pub(crate) fn current_matching_end(&self) -> Option<TokenId> {
        self.cursor.table_matching_end(self.peek_id())
    }

    /// `looksLikeName` (`type_info_impl.dart:487`) at cursor-relative offset `n`.
    pub(crate) fn looks_like_name_at(&self, n: usize) -> bool {
        Self::looks_like_name_kind(
            self.cursor.peek_kind_ahead(n),
            self.cursor.peek_kind_ahead(n + 1),
        )
    }

    pub(crate) fn looks_like_name_at_id(&self, id: TokenId) -> bool {
        let o = self.offset_of(id);
        Self::looks_like_name_kind(self.kind_at(o), self.kind_at(o + 1))
    }

    pub(crate) fn looks_like_name_kind(kind: TokenKind, next: TokenKind) -> bool {
        match kind {
            TokenKind::Identifier | TokenKind::Keyword(Keyword::This | Keyword::Super) => true,
            // `typedef` is a legal identifier, but `type typedef id` reads as
            // a separate declaration.
            TokenKind::Keyword(kw) => {
                is_identifier(kind) && (!matches!(kw, Keyword::Typedef) || !is_identifier(next))
            }
            _ => false,
        }
    }

    /// `looksLikeNameOrEndOfBlock` (`type_info_impl.dart:498`) at an id.
    pub(crate) fn looks_like_name_or_block_at_id(&self, id: TokenId) -> bool {
        let o = self.offset_of(id);
        let kind = self.kind_at(o);
        Self::looks_like_name_kind(kind, self.kind_at(o + 1))
            || kind == TokenKind::Eof
            || kind == TokenKind::CloseCurly
    }
}

// ---------------------------------------------------------------------------
// computeType (type_info.dart:158)
// ---------------------------------------------------------------------------

impl<L: ParserListener> Parser<'_, '_, L> {
    /// Official `computeType` at the cursor. Precondition: clean cursor
    /// (nothing queued).
    pub(crate) fn compute_type(&mut self, required: bool, in_declaration: bool) -> TypeInfo {
        let next = self.peek_kind();
        let next_next = self.cursor.peek_kind_ahead(1);
        if !is_valid_non_record_type_reference(next, next_next)
            && !self.peek_is_possible_record_type()
        {
            return self.compute_type_recovery(required, in_declaration);
        }

        if next == TokenKind::Keyword(Keyword::Void) {
            if self.is_generalized_function_type_at(1) {
                // `void Function(...)`.
                let mut complex = self.complex_from_here(TypeParamOrArg::None);
                complex.compute_rest(self, complex.start, required);
                if complex.gft_has_return_type.is_none() {
                    return TypeInfo::Void;
                }
                return TypeInfo::Complex(complex);
            }
            return TypeInfo::Void;
        }

        if self.is_generalized_function_type_at(0) {
            // `Function(...)` with no return type (computeNoTypeGFT,
            // type_info_impl.dart:747): computeRest is anchored at
            // beforeStart, one token before `Function`.
            let mut complex = self.complex_from_here(TypeParamOrArg::None);
            let from = complex.start.saturating_sub(1);
            complex.compute_rest(self, from, required);
            if complex.gft_has_return_type.is_none() {
                return if required {
                    TypeInfo::Simple
                } else {
                    TypeInfo::NoType
                };
            }
            return TypeInfo::Complex(complex);
        }

        if self.peek_is_possible_record_type() {
            return self.compute_record_type_shape(required);
        }

        // An identifier was seen (type_info.dart:263); cursor on the name.
        let type_param = self.compute_type_param_or_arg_at(1, in_declaration);
        if !matches!(type_param, TypeParamOrArg::None) {
            if let TypeParamOrArg::Simple1 { closer } = &type_param {
                if *closer != SimpleCloser::Gt {
                    // `id < id (`>=`|`>>`)`: the official decision reads the
                    // token after the *split* closer — the remainder `=`/`>`
                    // (splitGtEq/splitGtGt, type_info_impl.dart:1263/1293) —
                    // which is never `?`, never a name, and never a
                    // `Function` intro. Required keeps the fast shape (the
                    // non-nullable `simpleTypeWith1ArgumentGtEq/GtGt`),
                    // optional is unconditionally noType
                    // (type_info.dart:270-292); no virtual split of the real
                    // token table is needed.
                    return if required {
                        TypeInfo::SimpleWith1Arg { nullable: false }
                    } else {
                        TypeInfo::NoType
                    };
                }
                // `id<id>` fast shape: decide via what follows (type_info.dart:270-292).
                let closer_id = type_param.skip(self.peek_id());
                let mut follower = self.offset_of(closer_id) + 1;
                let has_question = self.kind_at(follower) == TokenKind::Question;
                if has_question {
                    follower += 1;
                }
                if !self.is_generalized_function_type_at_id(self.real_id(follower)) {
                    if required || self.looks_like_name_at_id(self.real_id(follower)) {
                        return TypeInfo::SimpleWith1Arg {
                            nullable: has_question,
                        };
                    }
                    return TypeInfo::NoType;
                }
            }
            let mut complex = self.complex_from_here(type_param);
            complex.compute_simple_with_type_args(self, required);
            // Optional-position follower test (computeSimpleWithTypeArguments,
            // type_info_impl.dart:1044-1046): for a raw compound closer the
            // official successor is the split remainder, which is never a
            // name, so the noType arm always fires.
            let follower_nameish =
                !complex.end_raw_compound && self.looks_like_name_or_block_at_id(complex.end + 1);
            if !required && complex.gft_has_return_type.is_none() && !follower_nameish {
                return TypeInfo::NoType;
            }
            return TypeInfo::Complex(complex);
        }

        // Plain identifier path (type_info.dart:305).
        if next_next == TokenKind::Period {
            return self.compute_prefixed_shape(required, in_declaration);
        }

        if self.is_generalized_function_type_at(1) {
            // `id Function(...)`.
            let mut complex = self.complex_from_here(TypeParamOrArg::None);
            complex.compute_rest(self, complex.start, required);
            if complex.gft_has_return_type.is_none() {
                return TypeInfo::Simple;
            }
            return TypeInfo::Complex(complex);
        }

        if next_next == TokenKind::Question {
            if self.is_generalized_function_type_at(2) {
                let mut complex = self.complex_from_here(TypeParamOrArg::None);
                complex.compute_rest(self, complex.start, required);
                if complex.gft_has_return_type.is_none() {
                    return TypeInfo::SimpleNullable;
                }
                return TypeInfo::Complex(complex);
            }
            if required || self.looks_like_name_at(2) {
                return TypeInfo::SimpleNullable;
            }
        } else if required || self.looks_like_name_at(1) {
            return TypeInfo::Simple;
        }
        TypeInfo::NoType
    }

    /// The `(...)` branch of `computeType` (type_info.dart:239-261).
    fn compute_record_type_shape(&mut self, required: bool) -> TypeInfo {
        let group_end = self.current_matching_end().expect("record group");
        let mut after = group_end + 1;
        if self.kind_at(self.offset_of(after)) == TokenKind::Question {
            after += 1;
        }
        if self.is_generalized_function_type_at_id(after) {
            let mut complex = self.complex_from_here(TypeParamOrArg::None);
            // computeRest starts at the `)` (official computeRest(start.endGroup!)).
            complex.compute_rest(self, group_end, required);
            if complex.gft_has_return_type.is_none() {
                // Not a GFT after all: it is a plain record type.
                let mut complex = self.complex_from_here(TypeParamOrArg::None);
                complex.compute_record_type(self, required);
                return TypeInfo::Complex(complex);
            }
            complex.gft_return_type_has_record_type = true;
            return TypeInfo::Complex(complex);
        }
        let mut complex = self.complex_from_here(TypeParamOrArg::None);
        complex.compute_record_type(self, required);
        TypeInfo::Complex(complex)
    }

    /// The `id.` branch of `computeType` (type_info.dart:307-353).
    fn compute_prefixed_shape(&mut self, required: bool, in_declaration: bool) -> TypeInfo {
        let cont = self.cursor.peek_kind_ahead(2);
        if is_valid_non_record_type_reference(cont, self.cursor.peek_kind_ahead(3)) {
            // The group is computed at the continuation token
            // (type_info.dart:319 computes at `next` after two steps), so
            // the `<` sits at cursor+3.
            let type_param = self.compute_type_param_or_arg_at(3, in_declaration);
            if matches!(type_param, TypeParamOrArg::None) {
                // Fast decision on what follows the continuation.
                let mut follower = 3;
                let has_question = self.cursor.peek_kind_ahead(3) == TokenKind::Question;
                if has_question {
                    follower += 1;
                }
                if !self.is_generalized_function_type_at(follower) {
                    if required || self.looks_like_name_at(follower) {
                        if !has_question {
                            return TypeInfo::Prefixed;
                        }
                        // `id.id?` falls through to the complex shape.
                    } else {
                        return TypeInfo::NoType;
                    }
                }
            }
            let mut complex = self.complex_from_here(type_param);
            complex.compute_prefixed_type(self, required);
            return TypeInfo::Complex(complex);
        }
        // `id.` non-identifier.
        if required {
            let type_param = self.compute_type_param_or_arg_at(2, in_declaration);
            let mut complex = self.complex_from_here(type_param);
            complex.compute_prefixed_type(self, required);
            complex.recovered = true;
            return TypeInfo::Complex(complex);
        }
        TypeInfo::NoType
    }

    /// The recovery-only front of `computeType` (type_info.dart:165-215):
    /// built-ins, `.`-prefixed orphans, and `var<...>` shapes.
    fn compute_type_recovery(&mut self, required: bool, in_declaration: bool) -> TypeInfo {
        let TokenKind::Keyword(kw) = self.peek_kind() else {
            return TypeInfo::NoType;
        };
        if kw.style() == dartq_scanner::KeywordStyle::BuiltIn {
            let type_param = self.compute_type_param_or_arg_at(1, in_declaration);
            if !matches!(type_param, TypeParamOrArg::None) {
                // The official `looksLikeName(typeParamOrArg.skip(next).next!)`
                // reads the split remainder for a compound closer (`=`/`>`),
                // which is never a name (type_info_impl.dart:1263/1293).
                let compound = matches!(
                    &type_param,
                    TypeParamOrArg::Simple1 {
                        closer: SimpleCloser::GtEq | SimpleCloser::GtGt
                    }
                );
                let skip_end = type_param.skip(self.peek_id());
                if required || (!compound && self.looks_like_name_at_id(skip_end + 1)) {
                    let mut complex = self.complex_from_here(type_param);
                    complex.end = skip_end;
                    complex.end_raw_compound = complex.type_args.ends_on_raw_compound();
                    complex.compute_rest(self, skip_end, required);
                    complex.recovered = true;
                    return TypeInfo::Complex(complex);
                }
            } else if required || self.is_generalized_function_type_at(1) {
                // Built-in as a type name, except member-introducer keywords.
                let is_decl_introducer = matches!(
                    kw,
                    Keyword::Get | Keyword::Set | Keyword::Factory | Keyword::Operator
                ) || (matches!(kw, Keyword::Typedef)
                    && is_identifier(self.cursor.peek_kind_ahead(1)));
                if !is_decl_introducer {
                    let mut complex = self.complex_from_here(TypeParamOrArg::None);
                    complex.end = complex.start;
                    complex.compute_rest(self, complex.start, required);
                    complex.recovered = true;
                    return TypeInfo::Complex(complex);
                }
            }
            return TypeInfo::NoType;
        }
        if required {
            if self.peek_kind() == TokenKind::Period {
                // Prefixed type missing its prefix.
                let type_param = self.compute_type_param_or_arg_at(1, in_declaration);
                let mut complex = self.complex_from_here(type_param);
                complex.compute_prefixed_type(self, required);
                complex.recovered = true;
                return TypeInfo::Complex(complex);
            }
            if kw == Keyword::Var
                && matches!(
                    self.cursor.peek_kind_ahead(1),
                    TokenKind::Lt | TokenKind::Comma | TokenKind::Gt
                )
            {
                let type_param = self.compute_type_param_or_arg_at(1, in_declaration);
                let skip_end = type_param.skip(self.peek_id());
                let mut complex = self.complex_from_here(type_param);
                complex.end = skip_end;
                complex.end_raw_compound = complex.type_args.ends_on_raw_compound();
                complex.compute_rest(self, complex.end, required);
                complex.recovered = true;
                return TypeInfo::Complex(complex);
            }
        }
        TypeInfo::NoType
    }

    /// Official `computeVariablePatternType` (`type_info.dart:392`): `TYPE as`
    /// / `TYPE when` resolves to `TYPE` being the pattern, not a type
    /// (sdk#52199).
    pub(crate) fn compute_variable_pattern_type(&mut self, required: bool) -> TypeInfo {
        let type_info = self.compute_type(required, false);
        if !matches!(type_info, TypeInfo::NoType) {
            let after = type_info.skip_type(self);
            let follower = self.kind_at(self.offset_of(after) + 1);
            if matches!(follower, TokenKind::Keyword(Keyword::As | Keyword::When)) {
                return TypeInfo::NoType;
            }
        }
        type_info
    }

    /// Official `computeTypeAfterIsOrAs` (`parser_impl.dart:9318`): required,
    /// with the nullable-`?` conditional disambiguation — a `?` followed by
    /// a hard follower keeps the nullable type, a `? {`/`? when` decides by
    /// conditional speculation, everything else drops the `?` so the
    /// conditional parses. Compute-only: the cursor is restored.
    pub(crate) fn compute_type_after_is_or_as(&mut self) -> TypeInfo {
        let before = self.cursor.mark();
        let type_info = self.compute_type(true, false);
        if !type_info.is_nullable() {
            return type_info;
        }
        let skip_end = type_info.skip_type(self);
        let follower = self.kind_at(self.offset_of(skip_end) + 1);
        let keep_nullable = matches!(
            follower,
            TokenKind::CloseParen
                | TokenKind::CloseCurly
                | TokenKind::CloseSquare
                | TokenKind::Question
                | TokenKind::QuestionQuestion
                | TokenKind::Comma
                | TokenKind::Semicolon
                | TokenKind::Colon
                | TokenKind::Keyword(Keyword::Is | Keyword::As)
                | TokenKind::PeriodPeriod
                | TokenKind::BarBar
                | TokenKind::AmpersandAmpersand
                | TokenKind::Eof
        );
        let decide_conditional = matches!(follower, TokenKind::OpenCurly)
            || follower == TokenKind::Keyword(Keyword::When);
        let result = if keep_nullable {
            type_info
        } else if decide_conditional && !self.can_parse_as_conditional_after(skip_end) {
            type_info
        } else {
            // `asNonNullable`; the complex shapes keep their `?` (documented
            // reduction — the fast shapes cover the pinned-corpus forms).
            match type_info {
                TypeInfo::SimpleNullable => TypeInfo::Simple,
                other => other,
            }
        };
        self.cursor.reset(before);
        result
    }

    /// `canParseAsConditional` from an absolute token id (the `?`): a
    /// mark/reset walk so the compute-only contract of this module holds.
    fn can_parse_as_conditional_after(&mut self, question: TokenId) -> bool {
        let before = self.cursor.mark();
        while self.cursor.peek_id() != question && !self.cursor.at_eof() {
            self.cursor.advance();
        }
        let result = self.can_parse_as_conditional();
        self.cursor.reset(before);
        result
    }

    /// `ComplexTypeInfo(beforeStart, typeArguments)` (`type_info_impl.dart:562`):
    /// `start` = `beforeStart.next!` — the type's FIRST token, which is where
    /// the cursor sits.
    fn complex_from_here(&self, type_args: TypeParamOrArg) -> ComplexType {
        let start = self.real_id(self.cursor.real_pos());
        let recovered = type_args.recovered();
        ComplexType {
            start,
            type_args,
            before_question: None,
            end: start,
            end_raw_compound: false,
            type_variable_starters: Vec::new(),
            gft_has_return_type: None,
            is_record_type: false,
            gft_return_type_has_record_type: false,
            recovered,
        }
    }
}
