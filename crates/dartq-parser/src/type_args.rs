//! Type-argument/type-parameter group machinery (official
//! `TypeParamOrArgInfo` from `type_info.dart` + `type_info_impl.dart`):
//! read-only group scanning (`scanTypeGroup`, `gtGroupLen`, record probe)
//! and the full-group consume paths (`parseArguments`/`parseVariables`).

use dartq_scanner::{Keyword, TokenKind};

use crate::cursor::CursorMark;
use crate::listener::IdentifierContext;
use crate::listener::ParserListener;
use crate::message::msg;
use crate::parser::Parser;
use crate::recovery::is_identifier;
use crate::token_id::{TokenId, TokenSpan};
use crate::types::{
    SimpleCloser, TypeInfo, is_closer, is_compound_closer, is_valid_non_record_type_reference,
    is_variance,
};

/// `TypeParamOrArgInfo` (`type_info.dart:80`).
#[derive(Clone, Debug)]
pub enum TypeParamOrArg {
    /// `NoTypeParamOrArg`.
    None,
    /// `<id>` fast shape with its closer kind.
    Simple1 { closer: SimpleCloser },
    /// Full `<...>` group; `end` is the skip target (a real id, possibly a
    /// combined `>>`-family token).
    Complex(ComplexTypeParamOrArg),
}

/// Full-group scan result payload (pure data; Copy by design).
#[derive(Clone, Copy, Debug)]
pub struct ComplexTypeParamOrArg {
    pub start: TokenId,
    pub end: TokenId,
    pub recovered: bool,
    /// `end` names a raw compound closer whose *leading* `>` closed this
    /// group (the official parser splits it there, `splitCloser`,
    /// `type_info_impl.dart:1772): the successor of `end` reads the split
    /// remainder, never a real token. `false` when the compound is shared
    /// with enclosing groups (its *last* closer closes this group, so the
    /// real successor follows).
    pub end_raw_compound: bool,
}

impl TypeParamOrArg {
    /// Official `recovered`.
    pub(crate) fn recovered(&self) -> bool {
        match self {
            TypeParamOrArg::Complex(g) => g.recovered,
            _ => false,
        }
    }

    /// Whether [`TypeParamOrArg::skip`] lands on a raw compound closer whose
    /// leading `>` closes the group (see [`ComplexTypeParamOrArg`]).
    pub(crate) fn ends_on_raw_compound(&self) -> bool {
        match self {
            // A compound closer on the fast shape is always raw: the official
            // `skipEndGroup` returns the split leading `>` (the compound's
            // position) and the remainder follows it.
            TypeParamOrArg::Simple1 { closer } => *closer != SimpleCloser::Gt,
            TypeParamOrArg::Complex(g) => g.end_raw_compound,
            TypeParamOrArg::None => false,
        }
    }

    /// `skip` (`type_info.dart:113)`: last token of the group, or `before`
    /// when absent. Returns real ids only.
    ///
    /// For a compound closer (`>=`/`>>`) this names the compound token —
    /// the position of the official *split* leading `>`
    /// (`splitGtEq`/`splitGtGt`, `type_info_impl.dart:1263/1293), whose
    /// successor is the remainder. The remainder is never a name, `?`, or a
    /// `Function` intro; decision sites special-case it instead of reading
    /// the next real token.
    pub(crate) fn skip(&self, before: TokenId) -> TokenId {
        match self {
            TypeParamOrArg::None => before,
            // `<` id closer — relative to `before` (the token before `<`):
            // the `<`, the argument, then the closer itself.
            TypeParamOrArg::Simple1 { .. } => before + 3,
            TypeParamOrArg::Complex(g) => g.end,
        }
    }

    /// Official `parseArguments`: consumes type arguments, emitting
    /// `begin/end_type_arguments` (or `handle_no_type_arguments`).
    ///
    /// Entry contract (both arms): the cursor sits on the `<` or on the
    /// official caller-pointer token one before it (the callee crosses the
    /// pointer / re-anchors, matching the official
    /// `beginGroup = token.next` / `Token next = start` bodies,
    /// `type_info_impl.dart:1192/:1430-1432`); the fast-shape guard
    /// guarantees exactly one nameish token before the `>`-family closer.
    pub(crate) fn parse_arguments(
        &self,
        parser: &mut Parser<'_, '_, impl ParserListener>,
    ) -> TokenSpan {
        match self {
            TypeParamOrArg::None => {
                let next = parser.peek_span();
                parser.listener.handle_no_type_arguments(next);
                next
            }
            TypeParamOrArg::Simple1 { .. } => {
                // Official entry: `beginGroup = token.next!` with
                // `beginGroup.isA(LT)` (`type_info_impl.dart:1192-1193`) —
                // the caller's pointer sits one before the `<`, and the
                // recovery paths that reach us deliver exactly that pointer
                // position as the live cursor (e.g. `parseMethodTypeArgs`
                // computes the group at `bangToken` and parses from
                // `bangToken.next`, `parser_impl.dart:7094-7102`). Cross the
                // pointer token when the cursor carries it so the `<` is
                // current either way.
                if parser.peek_kind() != TokenKind::Lt {
                    parser.cursor.advance();
                }
                debug_assert_eq!(parser.peek_kind(), TokenKind::Lt, "beginGroup.isA(LT)");
                if !parser.enter_depth() {
                    return parser.peek_span();
                }
                let begin = parser.peek_span(); // `<`
                parser.listener.begin_type_arguments(begin);
                parser.cursor.advance(); // `<`
                // Official `ensureIdentifier(token, typeReference)` — the
                // name token is consumed, not merely read.
                let name = parser.ensure_identifier(IdentifierContext::TypeReference);
                parser.listener.handle_type(name, None);
                // Official `parseEndGroup` asserts the scan-guaranteed
                // `>`-family closer (type_info_impl.dart:1243).
                debug_assert!(parser.cursor.peek_is_closer());
                let closer_id = parser.cursor.consume_closer();
                let closer = parser.span_of(closer_id);
                parser.listener.end_type_arguments(1, begin, closer);
                parser.leave_depth();
                closer
            }
            TypeParamOrArg::Complex(g) => (*g).parse_arguments(parser),
        }
    }

    /// Official `parseVariables`: consumes type parameters.
    pub(crate) fn parse_variables(
        &self,
        parser: &mut Parser<'_, '_, impl ParserListener>,
        allows_variance: bool,
    ) -> TokenSpan {
        match self {
            TypeParamOrArg::None => {
                let next = parser.peek_span();
                parser.listener.handle_no_type_variables(next);
                next
            }
            TypeParamOrArg::Simple1 { .. } => {
                // Official `SimpleTypeArgument1.parseVariables`
                // (`type_info_impl.dart:1204`): `<`, metadata star over the
                // name, the single variable, then the closer. Entry anchoring
                // mirrors the `parseArguments` arm above
                // (`beginGroup = token.next!`, :1204-1205).
                if parser.peek_kind() != TokenKind::Lt {
                    parser.cursor.advance();
                }
                debug_assert_eq!(parser.peek_kind(), TokenKind::Lt, "beginGroup.isA(LT)");
                if !parser.enter_depth() {
                    return parser.peek_span();
                }
                let begin = parser.peek_span(); // `<`
                parser.listener.begin_type_variables(begin);
                parser.cursor.advance(); // `<`
                parser.listener.begin_metadata_star(parser.peek_span());
                parser.listener.end_metadata_star(0);
                let name = parser.ensure_identifier(IdentifierContext::TypeVariableDeclaration);
                parser.listener.begin_type_variable(name);
                parser.listener.handle_type_variables_defined(name, 1);
                parser.listener.handle_no_type(name);
                debug_assert!(parser.cursor.peek_is_closer());
                let closer_id = parser.cursor.consume_closer();
                let end = parser.span_of(closer_id);
                parser.listener.end_type_variable(end, 0, None, None);
                parser.listener.end_type_variables(begin, end);
                parser.leave_depth();
                end
            }
            TypeParamOrArg::Complex(g) => (*g).parse_variables(parser, allows_variance),
        }
    }
}

impl<L: ParserListener> Parser<'_, '_, L> {
    /// Official `computeTypeParamOrArg` (`type_info.dart:420`): the cursor
    /// sits on the name/type token, so the `<` is one ahead.
    pub(crate) fn compute_type_param_or_arg(&mut self, in_declaration: bool) -> TypeParamOrArg {
        self.compute_type_param_or_arg_at(1, in_declaration)
    }

    /// Variant for callers whose cursor already sits on the `<` (metadata
    /// after its qualified name, GFT type-variable groups).
    pub(crate) fn compute_type_param_or_arg_on_lt(
        &mut self,
        in_declaration: bool,
    ) -> TypeParamOrArg {
        self.compute_type_param_or_arg_at(0, in_declaration)
    }

    pub(crate) fn compute_type_param_or_arg_at(
        &mut self,
        n: usize,
        in_declaration: bool,
    ) -> TypeParamOrArg {
        if self.cursor.peek_kind_ahead(n) != TokenKind::Lt {
            return TypeParamOrArg::None;
        }
        if let Some(closer) = simple1_closer(self, self.cursor.real_pos() + n) {
            return TypeParamOrArg::Simple1 { closer };
        }
        // `<(`: only a group when the parenthesized content is a record
        // type (type_info.dart:454-468).
        if self.cursor.peek_kind_ahead(n + 1) == TokenKind::OpenParen {
            let paren_id = self.real_id(self.cursor.real_pos() + n + 1);
            let paired = self
                .cursor
                .table_matching_end(paren_id)
                .filter(|end| !self.cursor.resolve(*end).1)
                .is_some_and(|end| {
                    RecordProbe {
                        open: self.offset_of(paren_id),
                    }
                    .looks_like_record_type(self, self.offset_of(end))
                });
            if !paired {
                return TypeParamOrArg::None;
            }
        }
        let start = self.real_id(self.cursor.real_pos() + n);
        match self.scan_type_group(start, in_declaration) {
            Some((end, recovered, end_raw_compound)) => {
                TypeParamOrArg::Complex(ComplexTypeParamOrArg {
                    start,
                    end,
                    recovered,
                    end_raw_compound,
                })
            }
            None => TypeParamOrArg::None,
        }
    }
}

/// The fast-shape guard of `_computeTypeParamOrArgImpl` (`type_info.dart:446)`:
/// `<` followed by a single name-ish token and a `>`-family closer.
fn simple1_closer(
    parser: &Parser<'_, '_, impl ParserListener>,
    lt_offset: usize,
) -> Option<SimpleCloser> {
    let first = parser.kind_at(lt_offset + 1);
    let first_is_nameish = is_identifier(first)
        || matches!(first, TokenKind::Keyword(kw) if kw.style() == dartq_scanner::KeywordStyle::Pseudo);
    if !first_is_nameish {
        return None;
    }
    match parser.kind_at(lt_offset + 2) {
        TokenKind::Gt => Some(SimpleCloser::Gt),
        TokenKind::GtGt => Some(SimpleCloser::GtGt),
        TokenKind::GtEq => Some(SimpleCloser::GtEq),
        _ => None,
    }
}

/// `>`-family closer count carried by one token (the unit mapping of
/// [`Parser::gt_group_len`]).
fn closer_units(kind: TokenKind) -> usize {
    match kind {
        TokenKind::Gt => 1,
        TokenKind::GtEq | TokenKind::GtGt => 2,
        TokenKind::GtGtEq | TokenKind::GtGtGt => 3,
        TokenKind::GtGtGtEq => 4,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// read-only group/record scanning
// ---------------------------------------------------------------------------

/// `(...)` record-type content probe (a reduced
/// `_checkIfRecordTypeParenthesisAreRecovered`).
pub(crate) struct RecordProbe {
    pub(crate) open: usize,
}

impl RecordProbe {
    /// Whether `(` at `self.open` (closing at `end_offset`) holds
    /// record-type field shapes.
    pub(crate) fn looks_like_record_type(
        &self,
        parser: &Parser<'_, '_, impl ParserListener>,
        end_offset: usize,
    ) -> bool {
        let mut offset = self.open;
        let mut fields = 0usize;
        let mut in_named = false;
        loop {
            offset += 1;
            if offset >= end_offset {
                break;
            }
            match parser.kind_at(offset) {
                TokenKind::CloseParen => break,
                TokenKind::OpenCurly if !in_named => {
                    in_named = true;
                    continue;
                }
                TokenKind::Comma => {
                    fields += 1;
                    continue;
                }
                _ => {}
            }
            let len = parser.skip_type_len(offset);
            if len == 0 {
                return false;
            }
            offset += len;
            fields += 1;
            if is_identifier(parser.kind_at(offset + 1)) {
                offset += 1;
            }
        }
        // A record needs a comma or named fields; one positional field
        // without a comma is a parenthesized type, not a record.
        fields >= 2 || in_named
    }
}

impl<L: ParserListener> Parser<'_, '_, L> {
    /// Scans a `<...>` group without consuming (official
    /// `ComplexTypeParamOrArgInfo.compute`). Returns
    /// `(closer id, recovered, closer is a raw compound)`. `scan` walks the
    /// real stream: it always names the next token to inspect.
    fn scan_type_group(
        &self,
        lt_id: TokenId,
        in_declaration: bool,
    ) -> Option<(TokenId, bool, bool)> {
        let lt_offset = self.offset_of(lt_id);
        let mut recovered = false;
        let mut scan = lt_offset + 1;
        loop {
            if self.kind_at(scan) == TokenKind::At {
                let metadata_len = self.metadata_skip_len(scan);
                if metadata_len == 0 {
                    return None;
                }
                scan += metadata_len;
            }
            let len = self.skip_type_len(scan);
            if len == 0 {
                let here = self.kind_at(scan);
                if here == TokenKind::Comma {
                    scan += 1;
                    continue;
                }
                if is_closer(here) {
                    // Stopped at the closer directly: a compound closer's
                    // leading `>` is this group's own (raw).
                    return Some((self.real_id(scan), recovered, is_compound_closer(here)));
                }
                if scan == lt_offset + 1 && !in_declaration {
                    return None;
                }
                recovered = true;
                return Some((self.real_id(scan), recovered, false));
            }
            let (end, mut backed_up) = self.step_over_type(scan, len);
            scan = end;
            if self.kind_at(scan) == TokenKind::Keyword(Keyword::Extends) {
                scan += 1;
                let super_len = self.skip_type_len(scan);
                let (end, super_backed) = self.step_over_type(scan, super_len);
                scan = end;
                backed_up = super_backed;
            }
            match self.kind_at(scan) {
                TokenKind::Comma => scan += 1,
                kind if is_closer(kind) => {
                    // A stop on a shared (backed-up-onto) compound closer is
                    // not raw: its last closer closes this group and the
                    // real successor follows.
                    return Some((
                        self.real_id(scan),
                        recovered,
                        is_compound_closer(kind) && !backed_up,
                    ));
                }
                _ => {
                    // Recovery: declaration-style content missing commas —
                    // stop at the first non-member shape.
                    recovered = true;
                    let mut probe = scan;
                    if self.kind_at(probe) == TokenKind::OpenParen
                        && let Some(end) = self.cursor.table_matching_end(self.real_id(probe))
                    {
                        probe = self.offset_of(end) + 1;
                    }
                    return Some((self.real_id(probe), recovered, false));
                }
            }
        }
    }

    /// Advance from a type's first token past its `skipType` length. When the
    /// type's last token is a compound closer with surplus closers, the
    /// type's own last group closed on the compound's leading `>` and the
    /// surplus belongs to the enclosing groups (the official parser splits
    /// the compound there, `splitCloser`, `util.dart:156`) — the scan stops
    /// ON the compound instead of stepping past it. Returns
    /// `(next offset, stopped on a shared compound)`.
    fn step_over_type(&self, offset: usize, len: usize) -> (usize, bool) {
        if len == 0 {
            return (offset, false);
        }
        let tail = self.kind_at(offset + len - 1);
        if is_compound_closer(tail) {
            // Open `<`s inside the type minus the closers they already
            // consumed: the depth the tail compound closes.
            let mut opens = 0usize;
            let mut closed = 0usize;
            for i in offset..offset + len - 1 {
                match self.kind_at(i) {
                    TokenKind::Lt => opens += 1,
                    kind => closed += closer_units(kind),
                }
            }
            if closer_units(tail) > opens.saturating_sub(closed) {
                return (offset + len - 1, true);
            }
        }
        (offset + len, false)
    }

    /// Length (in tokens) of the `<...>` group opening at `lt_offset`,
    /// counted by `<`/`>`-family balance (the scanner pairs at most one `<`
    /// per `>`-family token — `List<List<int>>` leaves the inner `<`
    /// unpaired — so the parser must count for itself). `None` at EOF.
    pub(crate) fn gt_group_len(&self, lt_offset: usize) -> Option<usize> {
        let mut depth = 1i32;
        let mut i = lt_offset;
        loop {
            i += 1;
            let closers = match self.kind_at(i) {
                TokenKind::Lt => {
                    depth += 1;
                    0
                }
                TokenKind::Gt => 1,
                TokenKind::GtEq | TokenKind::GtGt => 2,
                TokenKind::GtGtEq | TokenKind::GtGtGt => 3,
                TokenKind::GtGtGtEq => 4,
                TokenKind::Eof => return None,
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    // Balanced non-angle groups inside type arguments
                    // (record types, function type parameters): jump via
                    // the scanner-paired end when available.
                    match self.cursor.table_matching_end(self.real_id(i)) {
                        Some(end) => {
                            i = self.offset_of(end);
                            0
                        }
                        None => 0,
                    }
                }
                _ => 0,
            };
            if closers > 0 {
                depth -= closers;
                if depth <= 0 {
                    return Some(i - lt_offset + 1);
                }
            }
        }
    }

    /// `skipType` length (in tokens) of a type starting at real offset
    /// `offset`; `0` when nothing type-shaped is there. Read-only; used by
    /// group scans and record-type checks. Exact for valid input; recovery
    /// input may yield 0 where the official recovers (see module docs).
    pub(crate) fn skip_type_len(&self, offset: usize) -> usize {
        match self.kind_at(offset) {
            TokenKind::Keyword(Keyword::Void) => {
                let mut n = 1;
                if self.kind_at(offset + 1) == TokenKind::Lt
                    && let Some(len) = self.gt_group_len(offset + 1)
                {
                    n += len;
                    if self.kind_at(offset + n) == TokenKind::Question {
                        n += 1;
                    }
                }
                n
            }
            TokenKind::OpenParen => {
                // Record type `(...)` — only with a real (non-synthetic) pair.
                match self.cursor.table_matching_end(self.real_id(offset)) {
                    Some(end) if !self.cursor.resolve(end).1 => {
                        let mut n = self.offset_of(end) - offset + 1;
                        if self.kind_at(offset + n) == TokenKind::Question {
                            n += 1;
                        }
                        n
                    }
                    _ => 0,
                }
            }
            TokenKind::Keyword(Keyword::Function) => {
                let mut n = 1;
                if self.kind_at(offset + 1) == TokenKind::Lt {
                    let Some(len) = self.gt_group_len(offset + 1) else {
                        return 0;
                    };
                    n += len;
                }
                if self.kind_at(offset + n) != TokenKind::OpenParen {
                    return 0;
                }
                let Some(end) = self.cursor.table_matching_end(self.real_id(offset + n)) else {
                    return 0;
                };
                n = self.offset_of(end) - offset + 1;
                if self.kind_at(offset + n) == TokenKind::Question {
                    n += 1;
                }
                n
            }
            k if is_valid_non_record_type_reference(k, self.kind_at(offset + 1)) => {
                let mut n = 1;
                if self.kind_at(offset + 1) == TokenKind::Period
                    && is_valid_non_record_type_reference(
                        self.kind_at(offset + 2),
                        self.kind_at(offset + 3),
                    )
                {
                    n = 3;
                }
                if self.kind_at(offset + n) == TokenKind::Lt {
                    match self.gt_group_len(offset + n) {
                        Some(len) => {
                            // The group length covers only the `<...>` tail;
                            // the identifier (or `id.id`) prefix stays.
                            n += len;
                            // A compound closer whose surplus closers belong
                            // to the enclosing groups ends the type here
                            // (the official scan splits the compound at the
                            // inner group): the successor reads the split
                            // remainder — never a `?`, never a `Function`
                            // intro — so the type ends on the compound and
                            // the GFT chain below stays off
                            // (`List<List<int>> Function()` keeps its GFT on
                            // the OUTER type).
                            let tail = self.kind_at(offset + n - 1);
                            let shared = is_compound_closer(tail) && {
                                let mut opens = 0usize;
                                let mut closed = 0usize;
                                for i in offset..offset + n - 1 {
                                    match self.kind_at(i) {
                                        TokenKind::Lt => opens += 1,
                                        kind => closed += closer_units(kind),
                                    }
                                }
                                closer_units(tail) > opens.saturating_sub(closed)
                            };
                            if shared {
                                return n;
                            }
                            if self.kind_at(offset + n) == TokenKind::Question {
                                n += 1;
                            }
                        }
                        None => return 0,
                    }
                } else if self.kind_at(offset + n) == TokenKind::Question {
                    n += 1;
                }
                // Generalized-function-type suffixes: `computeType` classifies
                // `id Function(...)` (and chained suffixes) as one Complex
                // type whose `skipType` is the GFT's end (`computeRest`,
                // type_info_impl.dart:1016+; the official group scan is
                // compute-driven, type_info_impl.dart:1363-1388), so the skip
                // length covers each `<...>? (...)` suffix exactly when it is
                // GFT-shaped.
                while self.kind_at(offset + n) == TokenKind::Keyword(Keyword::Function) {
                    let mut probe = n + 1;
                    if self.kind_at(offset + probe) == TokenKind::Lt {
                        match self.gt_group_len(offset + probe) {
                            Some(len) => probe += len,
                            None => break,
                        }
                    }
                    if self.kind_at(offset + probe) != TokenKind::OpenParen {
                        break;
                    }
                    let Some(close) = self.cursor.table_matching_end(self.real_id(offset + probe))
                    else {
                        break;
                    };
                    n = self.offset_of(close) - offset + 1;
                    if self.kind_at(offset + n) == TokenKind::Question {
                        n += 1;
                    }
                }
                n
            }
            _ => 0,
        }
    }

    /// `skipMetadata` (util.dart:96) length at real offset `offset` (which
    /// points at the `@`); `0` when the shape does not match.
    pub(crate) fn metadata_skip_len(&self, offset: usize) -> usize {
        if self.kind_at(offset) != TokenKind::At || !is_identifier(self.kind_at(offset + 1)) {
            return 0;
        }
        let mut n = 2;
        if self.kind_at(offset + n) == TokenKind::Period
            && is_identifier(self.kind_at(offset + n + 1))
        {
            n += 2;
        }
        if self.kind_at(offset + n) == TokenKind::Lt
            && let Some(end) = self.cursor.table_matching_end(self.real_id(offset + n))
        {
            n = self.offset_of(end) - offset + 1;
        }
        if self.kind_at(offset + n) == TokenKind::Period
            && is_identifier(self.kind_at(offset + n + 1))
        {
            n += 2;
        }
        if self.kind_at(offset + n) == TokenKind::OpenParen
            && let Some(end) = self.cursor.table_matching_end(self.real_id(offset + n))
        {
            n = self.offset_of(end) - offset + 1;
        }
        n
    }
}

// ---------------------------------------------------------------------------
// full-group consume paths (ComplexTypeParamOrArgInfo.parseArguments/.parseVariables)
// ---------------------------------------------------------------------------

impl ComplexTypeParamOrArg {
    /// Official `ComplexTypeParamOrArgInfo.parseArguments`
    /// (`type_info_impl.dart:1428`). The body anchors on the scanned `start`
    /// (`assert(identical(token.next!, start))`, then
    /// `Token next = start;` — :1430-1432): the caller's `token` pointer is
    /// only ever asserted, never navigated. Recovery paths may deliver the
    /// live cursor still sitting on the pre-`<` pointer token, so re-anchor
    /// onto `start` before reading the group.
    pub(crate) fn parse_arguments(
        self,
        parser: &mut Parser<'_, '_, impl ParserListener>,
    ) -> TokenSpan {
        if parser.cursor.peek_id() != self.start {
            parser.cursor.rewind_to(self.start);
        }
        debug_assert_eq!(parser.cursor.peek_id(), self.start); // cursor on `<`
        let begin = parser.peek_span(); // `<`
        if !parser.enter_depth() {
            return begin;
        }
        parser.listener.begin_type_arguments(begin);
        parser.cursor.advance();
        let mut count = 0usize;
        let end;
        loop {
            if parser.is_bailed() {
                end = parser.peek_span();
                parser.listener.end_type_arguments(count, begin, end);
                parser.leave_depth();
                return end;
            }
            let before = parser.cursor.progress();
            let mut type_info = parser.compute_type(true, false);
            if matches!(type_info, TypeInfo::NoType) {
                // Recovery: annotations over type arguments are reported and
                // skipped without events (official skipMetadata probe).
                while matches!(type_info, TypeInfo::NoType) && parser.peek_kind() == TokenKind::At {
                    let at = parser.peek_span();
                    parser.cursor.advance();
                    parser.skip_metadata_silent();
                    type_info = parser.compute_type(true, false);
                    let end_span = parser.peek_span();
                    parser.report_with_end(msg::annotation_on_type_argument(), at, end_span);
                }
            }
            type_info.ensure_type_or_void(parser);
            count += 1;
            if parser.peek_kind() != TokenKind::Comma {
                if parser.cursor.peek_is_closer() {
                    end = {
                        let id = parser.cursor.consume_closer();
                        parser.span_of(id)
                    };
                    parser.listener.end_type_arguments(count, begin, end);
                    parser.leave_depth();
                    return end;
                }
                // Recovery: jump to the scanner-paired closer of this group.
                if let Some(group_end) = parser.cursor.table_matching_end(begin.id) {
                    parser.report(msg::expected_after_but_got(">"), parser.peek_span());
                    parser.skip_to_token(group_end);
                    end = {
                        let id = parser.cursor.advance();
                        parser.span_of(id)
                    };
                    parser.listener.end_type_arguments(count, begin, end);
                    parser.leave_depth();
                    return end;
                }
                // Recovery: report and keep going with a synthesized comma
                // (official). Progress guarantee: this arm runs only when no
                // closer exists anywhere ahead, so the next iteration must
                // examine a *new* real token — drain everything queued (the
                // synthetics' events are already emitted) and force one real
                // token forward when the iteration consumed none.
                parser.report(msg::expected_but_got(","), parser.peek_span());
                parser.synthetic(TokenKind::Comma, parser.peek_span().range.offset);
                while parser.cursor.peek_id() as usize >= parser.table.len() {
                    parser.cursor.advance();
                }
                if !parser.cursor.at_eof() && parser.cursor.progress() == before {
                    parser.force_advance();
                }
                continue;
            }
            parser.cursor.advance();
        }
    }

    /// Official `ComplexTypeParamOrArgInfo.parseVariables`
    /// (`type_info_impl.dart:1476`). Cursor sits before the `<`.
    pub(crate) fn parse_variables(
        self,
        parser: &mut Parser<'_, '_, impl ParserListener>,
        allows_variance: bool,
    ) -> TokenSpan {
        let begin = parser.peek_span(); // `<`
        debug_assert_eq!(parser.cursor.peek_id(), self.start); // entry invariant: cursor on `<`
        if !parser.enter_depth() {
            return begin;
        }
        parser.listener.begin_type_variables(begin);
        parser.cursor.advance();

        // Per-variable data, completed in reverse order at the end (the
        // official typeStarts/superTypeInfos/variances links).
        let mut starts: Vec<TokenSpan> = Vec::new();
        let mut supers: Vec<Option<(TokenSpan, TypeInfo)>> = Vec::new();
        let mut variances: Vec<Option<TokenSpan>> = Vec::new();

        loop {
            if parser.is_bailed() {
                break;
            }
            // Real-progress baseline for this variable iteration (the
            // missing-comma recovery arm compares against it).
            let iteration_before = parser.cursor.progress();
            parser.parse_metadata_star();

            // Variance modifier (`in`/`inout`/`out`) when allowed.
            let mut variance = None;
            if allows_variance && parser.peek_is_variance() && parser.peek_ahead_is_name(1) {
                let variance_span = parser.peek_span();
                variance = Some(variance_span);
                parser.cursor.advance();
                // Recovery: at most one variance modifier.
                while parser.peek_is_variance() && parser.peek_ahead_is_name(1) {
                    parser.report(msg::multiple_variance_modifiers(), parser.peek_span());
                    parser.cursor.advance();
                }
            }

            let name = parser.ensure_identifier(IdentifierContext::TypeVariableDeclaration);
            parser.listener.begin_type_variable(name);
            starts.push(name);

            if parser.peek_kind() == TokenKind::Keyword(Keyword::Extends) {
                let extends_span = parser.peek_span();
                parser.cursor.advance();
                let info = parser.compute_type(true, false);
                // Official `token = typeInfo.skipType(next)`
                // (type_info_impl.dart:1530): the scan position moves to the
                // supertype's last token. `NoType` keeps the official
                // `skipType` = the token before the type, i.e. no move.
                if !matches!(info, TypeInfo::NoType) {
                    let last = info.skip_type(parser);
                    parser.skip_to_token(last);
                    parser.cursor.advance();
                }
                supers.push(Some((extends_span, info)));
            } else {
                supers.push(None);
            }
            variances.push(variance);

            match parser.peek_kind() {
                TokenKind::Comma => {
                    parser.cursor.advance();
                }
                k if is_closer(k) => break,
                _ => {
                    // Recovery: keep going over parameter-like shapes with a
                    // synthesized comma, otherwise stop at the closer.
                    if parser.cursor.peek_is_closer() {
                        break;
                    }
                    if looks_like_type_param_member(parser) {
                        parser.report(msg::expected_but_got(","), parser.peek_span());
                        parser.synthetic(TokenKind::Comma, parser.peek_span().range.offset);
                        // Progress guarantee: drain the synthesized comma so
                        // the next iteration's `ensure_identifier` consumes
                        // the real name token, and force one token forward
                        // when the whole iteration consumed nothing real.
                        while parser.cursor.peek_id() as usize >= parser.table.len() {
                            parser.cursor.advance();
                        }
                        if !parser.cursor.at_eof() && parser.cursor.progress() == iteration_before {
                            parser.force_advance();
                        }
                        continue;
                    }
                    break;
                }
            }
        }

        let count = starts.len();
        parser
            .listener
            .handle_type_variables_defined(*starts.first().unwrap_or(&begin), count);

        // Type variables complete in reverse declaration order. Each bound
        // re-parses anchored at its own `extends` token (official
        // `typeInfo.ensureTypeNotVoid(next2, parser)` with
        // `next2 = typeStarts.head.next`, `type_info_impl.dart:1571`) — the
        // forward loop above only *skipped* the bound, so the live cursor
        // rewinds per bound. `closer_state` snapshots the first completion
        // step (= right after the last variable, the official `token3`
        // anchor, type_info_impl.dart:1565) so the group's closer handling
        // below reads the position the official parseCloser consumes from.
        let mut closer_state: Option<CursorMark> = None;
        for i in (0..count).rev() {
            let start = starts[i];
            let mut extends_or_super = None;
            match &supers[i] {
                Some((extends_span, info)) => {
                    extends_or_super = Some(*extends_span);
                    // Bounds sit earlier in the stream than anything a previous
                    // (later-variable) completion step queued, so the pending
                    // remainders detach for the rewind and re-attach after this
                    // bound's own splits.
                    let pending = if closer_state.is_some() {
                        Some(parser.cursor.detach_queue())
                    } else {
                        None
                    };
                    parser.cursor.rewind_to(extends_span.id + 1);
                    info.ensure_type_not_void(parser);
                    if let Some(pending) = pending {
                        parser.cursor.attach_queue(pending);
                    }
                }
                None => parser.listener.handle_no_type(start),
            }
            if closer_state.is_none() {
                closer_state = Some(parser.cursor.mark());
            }
            parser
                .listener
                .end_type_variable(start, count - 1 - i, extends_or_super, variances[i]);
        }
        if let Some(mark) = closer_state {
            parser.cursor.reset(mark);
        }

        let end = if parser.cursor.peek_is_closer() {
            let id = parser.cursor.consume_closer();
            parser.span_of(id)
        } else {
            parser.report(msg::expected_after_but_got(">"), parser.peek_span());
            parser.type_arg_group_end_or_synthetic(begin)
        };
        parser.listener.end_type_variables(begin, end);
        parser.leave_depth();
        end
    }
}
impl<L: ParserListener> Parser<'_, '_, L> {
    fn peek_is_variance(&self) -> bool {
        is_variance(self.peek_kind())
    }

    fn peek_ahead_is_name(&self, n: usize) -> bool {
        let kind = self.cursor.peek_kind_ahead(n);
        is_identifier(kind)
    }

    /// Recovery exit for a type-variable list whose closer is missing: jump
    /// to the scanner-paired `>` or synthesize one.
    pub(crate) fn type_arg_group_end_or_synthetic(&mut self, begin: TokenSpan) -> TokenSpan {
        if let Some(end) = self.cursor.table_matching_end(begin.id) {
            self.skip_to_token(end);
            return {
                let id = self.cursor.advance();
                self.span_of(id)
            };
        }
        let at = self.peek_span();
        self.synthetic(TokenKind::Gt, at.range.offset)
    }

    /// Consuming `skipMetadata` (util.dart:96): skips `@`-annotation-shaped
    /// tokens without emitting events. The cursor advances past the
    /// annotation when the shape matches.
    pub(crate) fn skip_metadata_silent(&mut self) {
        // Compute the length from the `@`'s offset, then consume it.
        let len = self.metadata_skip_len(self.cursor.real_pos());
        for _ in 0..len {
            if self.cursor.at_eof() {
                break;
            }
            self.cursor.advance();
        }
    }
}

/// `looksLikeTypeParamOrArg` (`type_info_impl.dart:512`), cursor flavor: the
/// current token continues a type parameter list after a missing comma.
fn looks_like_type_param_member(parser: &Parser<'_, '_, impl ParserListener>) -> bool {
    if !is_identifier(parser.peek_kind()) {
        return false;
    }
    matches!(
        parser.cursor.peek_kind_ahead(1),
        TokenKind::Identifier
            | TokenKind::Keyword(_)
            | TokenKind::Comma
            | TokenKind::Gt
            | TokenKind::GtGt
            | TokenKind::GtEq
            | TokenKind::GtGtGt
            | TokenKind::GtGtEq
            | TokenKind::GtGtGtEq
    )
}
