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
    /// `type_info_impl.dart:1772`): the successor of `end` reads the split
    /// remainder, never a real token. `false` when the compound is shared
    /// with enclosing groups (its *last* `>` closes this group, so the
    /// real successor follows).
    pub end_raw_compound: bool,
    /// The official `inDeclaration` field: it gates the missing-comma
    /// continuations (`looksLikeTypeParamOrArg`,
    /// `type_info_impl.dart:512-521`) in both the scan and the parse.
    pub in_declaration: bool,
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

    /// `skip` (`type_info.dart:113`): last token of the group, or `before`
    /// when absent. Returns real ids only.
    ///
    /// For a compound closer (`>=`/`>>`) this names the compound token —
    /// the position of the official *split* leading `>`
    /// (`splitGtEq`/`splitGtGt`, `type_info_impl.dart:1263/1293`), whose
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
        parser: &mut Parser<'_, '_, '_, impl ParserListener>,
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
                if !parser.enter_type_depth() {
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
                parser.leave_type_depth();
                closer
            }
            TypeParamOrArg::Complex(g) => (*g).parse_arguments(parser),
        }
    }

    /// Official `parseVariables`: consumes type parameters.
    pub(crate) fn parse_variables(
        &self,
        parser: &mut Parser<'_, '_, '_, impl ParserListener>,
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
                if !parser.enter_type_depth() {
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
                parser.leave_type_depth();
                end
            }
            TypeParamOrArg::Complex(g) => (*g).parse_variables(parser, allows_variance),
        }
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
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
        if self.cursor.look_kind(n) != TokenKind::Lt {
            return TypeParamOrArg::None;
        }
        if let Some(closer) = simple1_closer(self, self.cursor.real_pos() + n) {
            return TypeParamOrArg::Simple1 { closer };
        }
        // `<(`: only a group when `computeType(required: false)` commits an
        // unrecovered record shape (type_info.dart:454-468) — the
        // `computeRecordType` `!required` follower veto
        // (`type_info_impl.dart:771-860`) is what keeps `<(int, int)>[]` a
        // typed list literal while `<(int, int) x>[]` / `<(int, int)
        // async>[]` / `<(int, int) Function>[]` are not type arguments at
        // all (the follower `>` never accepts those shapes).
        if self.cursor.look_kind(n + 1) == TokenKind::OpenParen {
            let paren_id = self.real_id(self.cursor.real_pos() + n + 1);
            let record = self
                .cursor
                .table_matching_end(paren_id)
                .filter(|end| !self.cursor.resolve(*end).1)
                .is_some_and(|_| {
                    let before = self.cursor.mark();
                    self.cursor.jump_to(paren_id);
                    let verdict = match self.compute_type(false, false) {
                        TypeInfo::Complex(c) => {
                            (c.is_record_type || c.gft_return_type_has_record_type) && !c.recovered
                        }
                        _ => false,
                    };
                    self.cursor.reset(before);
                    verdict
                });
            if !record {
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
                    in_declaration,
                })
            }
            None => TypeParamOrArg::None,
        }
    }
}

/// The fast-shape guard of `_computeTypeParamOrArgImpl` (`type_info.dart:446)`:
/// `<` followed by a single name-ish token and a `>`-family closer.
pub(crate) fn simple1_closer(
    parser: &Parser<'_, '_, '_, impl ParserListener>,
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
pub(crate) fn closer_units(kind: TokenKind) -> usize {
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

/// `(...)` record-type content probe (the reduced
/// `_checkIfRecordTypeParenthesisAreRecovered`,
/// `type_info_impl.dart:882-943`).
pub(crate) struct RecordProbe {
    pub(crate) open: usize,
}

impl RecordProbe {
    /// Whether `(` at `self.open` (closing at `end_offset`) holds
    /// record-type field shapes. Faithful walk of the official check: each
    /// field is an optional named-section `{`, optional metadata, a type,
    /// and one optional name (a name only directly after the type — the
    /// next field's type must never read as a name), followed by `,`,
    /// `)`, or the named section's closing `}` + `)`. The tail verdict
    /// rejects a single positional field with no comma (`(int)` is not a
    /// record) while `(int,)` and `()` pass.
    pub(crate) fn looks_like_record_type(
        &self,
        parser: &Parser<'_, '_, '_, impl ParserListener>,
        end_offset: usize,
    ) -> bool {
        // Walk head: the last consumed separator (`(`, `,`, `{`).
        let mut token = self.open;
        let mut has_named_fields = false;
        let mut parameter_count = 0usize;
        let mut has_comma = false;
        loop {
            let next = token + 1;
            // The `)` arm consumes next == end_offset; only a walk that
            // overshoots the paired group (impossible over balanced table
            // spans) hits this bound.
            if next > end_offset {
                return false;
            }
            match parser.kind_at(next) {
                TokenKind::CloseParen => break,
                TokenKind::CloseCurly
                    if has_named_fields && parser.kind_at(next + 1) == TokenKind::CloseParen =>
                {
                    break;
                }
                _ => {}
            }
            parameter_count += 1;
            if !has_named_fields && parser.kind_at(next) == TokenKind::OpenCurly {
                has_named_fields = true;
                token = next;
            }
            let mut type_at = token + 1;
            if parser.kind_at(type_at) == TokenKind::At {
                let meta_len = parser.metadata_skip_len(type_at);
                if meta_len == 0 {
                    return false;
                }
                type_at += meta_len;
            }
            let len = parser.skip_type_len(type_at);
            if len == 0 {
                return false; // official: computeType(required).recovered
            }
            token = type_at + len - 1;
            if is_identifier(parser.kind_at(token + 1)) {
                token += 1; // optional field name
            } else if has_named_fields {
                return false; // named fields require names
            }
            match parser.kind_at(token + 1) {
                TokenKind::Comma => {
                    has_comma = true;
                    token += 1;
                }
                TokenKind::CloseParen => break,
                TokenKind::CloseCurly if parser.kind_at(token + 2) == TokenKind::CloseParen => {
                    break;
                }
                _ => return false, // recovery
            }
        }
        // Official tail verdict (type_info_impl.dart:937-941): exactly one
        // positional field with no comma is a parenthesized type, not a
        // record; the empty record and trailing-comma singles pass.
        !(parameter_count == 1 && !has_named_fields && !has_comma)
    }
}

impl<L: ParserListener> Parser<'_, '_, '_, L> {
    /// Official `parseUnexpectedEnd` (`type_info_impl.dart:1607-1758`) for a
    /// recovered type-ARGUMENT group: runs when neither a `,` nor the group
    /// closer follows the argument type. Arms in official order; the report
    /// anchor is the last consumed type end walked off zero-width synthetics
    /// (the official `reportRecoverableError` `findNonZeroLengthToken`), and
    /// `error_reported` mirrors the official `token.isSynthetic` gate (the
    /// ensure path's `expected_type_name` row already covers the group), so
    /// at most one `expected_after_but_got('>')` fires per group. `begin` is
    /// the group's own `<`. Returns the group-end span: the consumed
    /// `>`-family closer, or a synthesized `>` when no real closer absorbs
    /// the junk.
    ///
    /// Junk-tail postures, in official order (:1708-1722): a real closer
    /// directly after the junk absorbs it (`parseCloser(next)`); otherwise
    /// the walk-to-endGroup branch — the group's `<` carries the
    /// scanner-paired real closer (`start.endGroup`, the scanner's
    /// `>`-matching stack, abstract_scanner.dart:624-631), and a closer at
    /// or beyond the junk absorbs the whole junk run silently and closes
    /// the group there; only a cursor already past that closer (the
    /// `charOffset <= endGroup.charOffset` guard) keeps the synthetic-`>`
    /// posture with the junk left current for the enclosing recovery layer.
    ///
    /// The `< } >`-shaped junk face (the pinned `issue_39060` flow): the
    /// junk's follower is a real `>`, so the fallback's `parseCloser(next)`
    /// arm absorbs the junk silently and the group closes on the real
    /// closer — the oracle emits no per-junk rows, and the enclosing
    /// statement's semicolon face anchors on the closer.
    fn parse_type_args_unexpected_end(
        &mut self,
        in_declaration: bool,
        mut error_reported: bool,
        begin: TokenSpan,
    ) -> TokenSpan {
        // `extends` type continuation (official :1614-1629).
        if self.peek_kind() == TokenKind::Keyword(Keyword::Extends) {
            if !error_reported {
                let at = self.last_consumed_anchor();
                self.report(msg::expected_after_but_got(">"), at);
                error_reported = true;
            }
            self.cursor.advance();
            if is_valid_non_record_type_reference(self.peek_kind(), self.cursor.look_kind(1)) {
                let type_info = self.compute_type(true, false);
                // Silent stream-shaping: the official re-parse runs under a
                // ForwardingListener (events dropped); the diet skip consumes
                // the same real span.
                type_info.skip_type_raw(self);
            }
            if self.cursor.peek_is_closer() {
                let id = self.cursor.consume_closer();
                return self.span_of(id);
            }
        }
        // dynamic / void / `Function` type continuation (official :1631-1656).
        if matches!(
            self.peek_kind(),
            TokenKind::Keyword(Keyword::Dynamic | Keyword::Void | Keyword::Function)
        ) {
            let type_info = self.compute_type(true, false);
            if !matches!(type_info, TypeInfo::NoType) {
                if !error_reported {
                    let at = self.last_consumed_anchor();
                    self.report(msg::expected_after_but_got(">"), at);
                    error_reported = true;
                }
                type_info.skip_type_raw(self);
                if self.cursor.peek_is_closer() {
                    let id = self.cursor.consume_closer();
                    return self.span_of(id);
                }
            }
        }
        // A nested type-argument group continuation (official :1658-1690).
        let nested = self.compute_type_param_or_arg(in_declaration);
        if !matches!(nested, TypeParamOrArg::None) {
            if !error_reported {
                let at = self.last_consumed_anchor();
                self.report(msg::expected_after_but_got(">"), at);
                error_reported = true;
            }
            // Silent diet skip over the nested group (the official re-parse
            // runs under a ForwardingListener).
            let end_id = nested.skip(self.peek_id());
            self.skip_to_token(end_id);
            self.cursor.advance();
            if self.cursor.peek_is_closer() {
                let id = self.cursor.consume_closer();
                return self.span_of(id);
            }
        }
        // A `(...)` group continuation (official :1692-1706).
        if self.peek_kind() == TokenKind::OpenParen
            && let Some(close) = self.current_matching_end()
        {
            if !error_reported {
                let at = self.last_consumed_anchor();
                self.report(msg::expected_after_but_got(">"), at);
                error_reported = true;
            }
            self.skip_to_token(close);
            self.cursor.advance();
            if self.cursor.peek_is_closer() {
                let id = self.cursor.consume_closer();
                return self.span_of(id);
            }
        }
        // Fallback (official :1708-1722): one group-level row; then a real
        // closer directly after the junk absorbs it (`parseCloser(next)`),
        // else the scanner-paired endGroup walk absorbs the junk run and
        // closes the group on the real closer, else the group closes on a
        // synthesized `>` and the junk stays current for the enclosing
        // recovery layer.
        if !error_reported {
            let at = self.last_consumed_anchor();
            self.report(msg::expected_after_but_got(">"), at);
        }
        if is_closer(self.cursor.look_kind(1)) {
            self.cursor.advance();
            let id = self.cursor.consume_closer();
            return self.span_of(id);
        }
        if let Some(end_group) = self.cursor.table_matching_end(begin.id) {
            // Official guard: `token.charOffset <= endGroup.charOffset`
            // (type_info_impl.dart:1713-1717) — both sides are UTF-16 char
            // offsets, so the closer contributes its span offset, never its
            // token-table index.
            let end_offset = self.cursor.span(end_group).offset;
            while !self.cursor.at_eof()
                && self.cursor.peek_id() != end_group
                && self.peek_span().range.offset <= end_offset
            {
                self.cursor.advance();
            }
        }
        if self.cursor.peek_is_closer() {
            let id = self.cursor.consume_closer();
            return self.span_of(id);
        }
        let offset = self.peek_span().range.offset;
        let id = self.cursor.insert_synthetic(TokenKind::Gt, offset);
        self.cursor.advance();
        self.span_of(id)
    }

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
                // Official computeType(required) recovery: `var` as a type
                // entry, accepted before `<`/`,`/`>` (type_info.dart:210-217,
                // `computeBuiltinOrVarAsType`); the parse reports
                // `var_as_type_name` through the type-reference ensure.
                if here == TokenKind::Keyword(Keyword::Var)
                    && matches!(
                        self.kind_at(scan + 1),
                        TokenKind::Lt | TokenKind::Comma | TokenKind::Gt
                    )
                {
                    recovered = true;
                    scan += 1;
                    continue;
                }
                if scan == lt_offset + 1 && !in_declaration {
                    return None;
                }
                return Some(self.recovered_group_end(scan));
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
                    // Official :1395-1397: outside a declaration a member
                    // followed by neither `,` nor a closer means this is no
                    // type-argument group at all (`Map<String, int m()` in
                    // a block reads as relational expressions).
                    if !in_declaration {
                        return None;
                    }
                    // Official missing-comma continuation
                    // (type_info_impl.dart:1399-1404): in a declaration an
                    // identifier followed by another identifier, `,`, or a
                    // closer reads as a missing comma — the junk is stepped
                    // over as its own argument and the scan stays clean (no
                    // recovered flag; `looksLikeTypeParamOrArg`,
                    // :512-521, reads the raw IDENTIFIER kind, so
                    // pseudo-keyword junk like `async` never qualifies).
                    if in_declaration
                        && self.kind_at(scan) == TokenKind::Identifier
                        && (matches!(
                            self.kind_at(scan + 1),
                            TokenKind::Identifier | TokenKind::Comma
                        ) || is_closer(self.kind_at(scan + 1)))
                    {
                        let len = self.skip_type_len(scan);
                        if len > 0 {
                            scan += len;
                            continue;
                        }
                    }
                    // Recovery: declaration-style content missing commas —
                    // stop at the first non-member shape.
                    return Some(self.recovered_group_end(scan));
                }
            }
        }
    }

    /// The recovery tail of `ComplexTypeParamOrArgInfo.compute`
    /// (type_info_impl.dart:1408-1423) at the junk offset `next`: a `(`
    /// group is stepped over, then a closer at the follower or one token
    /// further ends the group (`splitCloser`), else the group ends on a
    /// synthetic `>` in front of the follower (`syntheticGt`) — encoded as
    /// the token before the follower being the group's last token, so the
    /// type ends there and the follower (a declaration's name) stays out of
    /// the type.
    fn recovered_group_end(&self, mut next: usize) -> (TokenId, bool, bool) {
        if self.kind_at(next) == TokenKind::OpenParen
            && let Some(end) = self.cursor.table_matching_end(self.real_id(next))
        {
            next = self.offset_of(end) + 1;
        }
        for at in [next, next + 1] {
            let kind = self.kind_at(at);
            if is_closer(kind) {
                return (self.real_id(at), true, is_compound_closer(kind));
            }
        }
        (self.real_id(next - 1), true, false)
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
                // `void Function(...)` — a generalized function type with a
                // `void` return type (the official group scan walks this via
                // the full `computeType`, `type_info_impl.dart:1356`).
                if self.kind_at(offset + n) == TokenKind::Keyword(Keyword::Function) {
                    let fn_len = self.skip_type_len(offset + n);
                    if fn_len > 0 {
                        n += fn_len;
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
                        // `(...) Function(...)` — a generalized function type
                        // with a record (or nullable-record) return type
                        // (official computeRecordTypeGFT /
                        // computeRecordTypeQuestionGFT,
                        // type_info_impl.dart:982/:1019, whose computeRest
                        // from the `)` ends past the formal parameters).
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
                            let Some(close) =
                                self.cursor.table_matching_end(self.real_id(offset + probe))
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
            TokenKind::Keyword(Keyword::Function) => {
                let mut n = 1;
                if self.kind_at(offset + 1) == TokenKind::Lt {
                    let Some(len) = self.gt_group_len(offset + 1) else {
                        return 0;
                    };
                    n += len;
                }
                if self.kind_at(offset + n) == TokenKind::OpenParen {
                    let Some(end) = self.cursor.table_matching_end(self.real_id(offset + n)) else {
                        return 0;
                    };
                    n = self.offset_of(end) - offset + 1;
                    if self.kind_at(offset + n) == TokenKind::Question {
                        n += 1;
                    }
                } else {
                    // Bare `Function` head — not a GFT intro: the official
                    // computeType(required) reads it as `simpleType` /
                    // `simpleNullableType` (type_info.dart:305-330), so the
                    // skip covers the head plus its nullable `?`. Without
                    // this arm a type-argument entry like `List<Function?>`
                    // scanned as length 0, the whole `<...>` group was
                    // rejected, and the `?` fell to the expression layer as
                    // a conditional (the expected-token ':' cascade).
                    if self.kind_at(offset + n) == TokenKind::Question {
                        n += 1;
                    }
                }
                // computeRest loops the whole `Function <...>? (...) ?`
                // suffix chain (`Function() Function()` is ONE type,
                // type_info_impl.dart:1094-1133); the identifier and record
                // arms below mirror the same loop.
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
        parser: &mut Parser<'_, '_, '_, impl ParserListener>,
    ) -> TokenSpan {
        if parser.cursor.peek_id() != self.start {
            parser.cursor.rewind_to(self.start);
        }
        debug_assert_eq!(parser.cursor.peek_id(), self.start); // cursor on `<`
        let begin = parser.peek_span(); // `<`
        if !parser.enter_type_depth() {
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
                parser.leave_type_depth();
                return end;
            }
            let mut type_info = parser.compute_type(true, false);
            // Recovery: annotations over type arguments are reported and
            // skipped without events (official skipMetadata probe; the
            // cursor sits on the `@` — official `skipMetadata(next)`
            // consumes `next.next!`). The bail check keeps the skip loop
            // structurally forward when the recompute latched the cap.
            while matches!(type_info, TypeInfo::NoType)
                && parser.peek_kind() == TokenKind::At
                && !parser.is_bailed()
            {
                let at = parser.peek_span();
                parser.skip_metadata_silent();
                type_info = parser.compute_type(true, false);
                let end_span = parser.peek_span();
                parser.report_with_end(msg::annotation_on_type_argument(), at, end_span);
            }
            type_info.ensure_type_or_void(parser);
            // Official `next = token.next!`: the NoType ensure path returns
            // the inserted synthetic identifier as the consumed type, so the
            // live pointer is its real follower (`,`/`>`), never the
            // synthetic. Draining the queued synthetic keeps the comma and
            // closer decisions on the real stream (a synthetic peek minted a
            // spurious `Expected '>' after this.` for `Map<, int>` /
            // `Map<int, >` / `f<>()`).
            if parser.cursor.peek().is_synthetic {
                parser.cursor.advance();
            }
            count += 1;
            if parser.peek_kind() != TokenKind::Comma {
                if parser.cursor.peek_is_closer() {
                    end = {
                        let id = parser.cursor.consume_closer();
                        parser.span_of(id)
                    };
                    parser.listener.end_type_arguments(count, begin, end);
                    parser.leave_type_depth();
                    return end;
                }
                // Official `looksLikeTypeParamOrArg` + `parseMissingComma`
                // (type_info_impl.dart:1462-1467, :512-521, :1595-1604): a
                // real identifier followed by another identifier, `,`, or a
                // closer reads as a missing comma — the report anchors on
                // the junk token itself, a synthetic comma splices in, and
                // the loop keeps parsing arguments.
                if self.in_declaration
                    && parser.peek_kind() == TokenKind::Identifier
                    && (matches!(
                        parser.cursor.look_kind(1),
                        TokenKind::Identifier | TokenKind::Comma
                    ) || is_closer(parser.cursor.look_kind(1)))
                {
                    let at = parser.peek_span();
                    parser.report(msg::expected_but_got(","), at);
                    parser.synthetic(TokenKind::Comma, at.range.offset);
                    while parser.cursor.peek_id() as usize >= parser.table.len() {
                        parser.cursor.advance();
                    }
                    if parser.cursor.at_eof() {
                        end = parser.peek_span();
                        parser.listener.end_type_arguments(count, begin, end);
                        parser.leave_type_depth();
                        return end;
                    }
                    continue;
                }
                // Official `parseUnexpectedEnd` (`type_info_impl.dart:1607`):
                // the recovered-group tail, in official arm order.
                // `error_reported` mirrors the official `token.isSynthetic`
                // gate — the ensure path's `expected_type_name` row already
                // covers the group — so at most one
                // `expected_after_but_got('>')` fires.
                let error_reported = matches!(type_info, TypeInfo::NoType);
                end = parser.parse_type_args_unexpected_end(
                    self.in_declaration,
                    error_reported,
                    begin,
                );
                parser.listener.end_type_arguments(count, begin, end);
                parser.leave_type_depth();
                return end;
            }
            parser.cursor.advance();
        }
    }

    /// Official `ComplexTypeParamOrArgInfo.parseVariables`
    /// (`type_info_impl.dart:1476`). Cursor sits before the `<`.
    pub(crate) fn parse_variables(
        self,
        parser: &mut Parser<'_, '_, '_, impl ParserListener>,
        allows_variance: bool,
    ) -> TokenSpan {
        let begin = parser.peek_span(); // `<`
        debug_assert_eq!(parser.cursor.peek_id(), self.start); // entry invariant: cursor on `<`
        if !parser.enter_type_depth() {
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

            // Variance modifier (`in`/`inout`/`out`) when allowed. The
            // modifier parses regardless of the experiment state (official
            // `parseVariables` has no feature gate); the analyzer reports
            // `experiment_not_enabled_off_by_default` per kept modifier from
            // `endTypeVariable` (ast_builder.dart:3604).
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
                if !parser.features.variance_enabled {
                    parser.report(
                        msg::experiment_not_enabled_off_by_default("variance"),
                        variance_span,
                    );
                }
            }

            let name = parser.ensure_identifier(IdentifierContext::TypeVariableDeclaration);
            parser.listener.begin_type_variable(name);
            starts.push(name);

            // Official `isCloser(token)` (type_info_impl.dart:1539): the
            // break tests the BOUND's last token, not the follower — a
            // bound ending on a `>`-family token (e.g. `T extends C<D>>`
            // where the shared `>>` also closes this group) ends the
            // variable list; the completion pass re-parses the bound and
            // the tail's `consume_closer` takes the split remainder as the
            // group's own closer.
            let mut bound_ends_on_closer = false;
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
                    bound_ends_on_closer = is_closer(parser.kind_at(parser.offset_of(last)));
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
                _ if bound_ends_on_closer => break,
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
        parser.leave_type_depth();
        end
    }
}
impl<L: ParserListener> Parser<'_, '_, '_, L> {
    fn peek_is_variance(&self) -> bool {
        is_variance(self.peek_kind())
    }

    fn peek_ahead_is_name(&self, n: usize) -> bool {
        let kind = self.cursor.look_kind(n);
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
fn looks_like_type_param_member(parser: &Parser<'_, '_, '_, impl ParserListener>) -> bool {
    // Official `looksLikeTypeParamOrArg(inDeclaration, next)`
    // (type_info_impl.dart:1417-1421): a `<`-headed group after a variable
    // reads as a missing comma — "Looks like missing comma. Continue
    // looping." — not as the group end (`typedef <A<B>> = x;` walks `A`,
    // `<`, `B` as recovered variables before the shared closer).
    if parser.peek_kind() == TokenKind::Lt && is_identifier(parser.cursor.look_kind(1)) {
        return true;
    }
    if !is_identifier(parser.peek_kind()) {
        return false;
    }
    matches!(
        parser.cursor.look_kind(1),
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
