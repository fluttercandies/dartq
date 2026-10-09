//! Literal collection/string/symbol parsing (reduced
//! `parseLiteralListSuffix` / `parseLiteralSetOrMapSuffix` /
//! `parseParenthesizedOrRecordLiteral` / string interpolation / symbols).
//! Control-flow elements (if/for/spread) are milestone #20.

use dartiq_scanner::TokenKind;

use crate::expr_const::ConstantPatternContext;
use crate::expr_const::is_user_definable_operator;
use crate::listener::{IdentifierContext, ParserListener};
use crate::message::msg;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

impl<L: ParserListener> Parser<'_, '_, L> {
    /// elements (if/for/spread) are milestone #20).
    pub(crate) fn parse_list_literal(&mut self, const_keyword: Option<TokenSpan>) -> TokenSpan {
        let left = self.peek_span();
        self.cursor.advance();
        let mut count = 0usize;
        let end;
        loop {
            if self.peek_kind() == TokenKind::CloseSquare {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if self.peek_kind() == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            self.parse_constant_expression(ConstantPatternContext::None);
            count += 1;
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseSquare => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                TokenKind::Eof => {
                    end = self.peek_span();
                    break;
                }
                _ => {
                    // Recovery (`parseLiteralListSuffix` :8545-8560): an
                    // expression-looking token means a missing comma — report
                    // and continue; otherwise jump to the group's closer,
                    // silently when the scanner synthesized it.
                    if crate::patterns::looks_like_literal_entry(self, self.cursor.real_pos()) {
                        self.report(msg::expected_but_got(","), self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        continue;
                    }
                    if let Some(close) = self.current_matching_end_for_open(left) {
                        let (_, synthetic) = self.cursor.resolve(close);
                        if !synthetic {
                            self.report(msg::expected_but_got("]"), self.peek_span());
                        }
                        self.skip_to_token(close);
                        end = self.peek_span();
                        self.cursor.advance();
                    } else {
                        self.report(msg::expected_but_got("]"), self.peek_span());
                        let at = self.peek_span();
                        end = self.synthetic(TokenKind::CloseSquare, at.range.offset);
                    }
                    break;
                }
            }
        }
        self.listener
            .handle_literal_list(count, left, const_keyword, end);
        end
    }

    /// Set/map literal (reduced `parseLiteralSetOrMapSuffix` :8568).
    pub(crate) fn parse_set_or_map_literal(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let left = self.peek_span();
        self.cursor.advance();
        let mut count = 0usize;
        // Official `hasSetEntry ??=` (:8596): FIRST-wins — only the first
        // entry decides whether `{...}` leans set.
        let mut has_set_entry: Option<bool> = None;
        let end;
        loop {
            if self.peek_kind() == TokenKind::CloseCurly {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            if self.peek_kind() == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            // Entry vs element in a single pass (official :8589-8618): parse
            // the expression once, then branch on `:`.
            self.parse_constant_expression(ConstantPatternContext::None);
            if self.peek_kind() == TokenKind::Colon {
                let colon = self.peek_span();
                self.cursor.advance();
                self.parse_constant_expression(ConstantPatternContext::None);
                let entry_end = self.peek_span();
                self.listener
                    .handle_literal_map_entry(colon, entry_end, None, None);
                has_set_entry.get_or_insert(false);
            } else {
                has_set_entry.get_or_insert(true);
            }
            count += 1;
            match self.peek_kind() {
                TokenKind::Comma => {
                    self.cursor.advance();
                }
                TokenKind::CloseCurly => {
                    end = self.peek_span();
                    self.cursor.advance();
                    break;
                }
                TokenKind::Eof => {
                    end = self.peek_span();
                    break;
                }
                _ => {
                    // Recovery (`parseLiteralSetOrMapSuffix` :8629-8660): an
                    // expression-looking token means a missing comma — report,
                    // insert it and continue; otherwise report and jump to
                    // where the scanner guaranteed the `}`.
                    if crate::patterns::looks_like_literal_entry(self, self.cursor.real_pos()) {
                        self.report(msg::expected_but_got(","), self.peek_span());
                        self.synthetic(TokenKind::Comma, self.peek_span().range.offset);
                        continue;
                    }
                    self.report(msg::expected_but_got("}"), self.peek_span());
                    if let Some(close) = self.current_matching_end_for_open(left) {
                        self.skip_to_token(close);
                        end = self.peek_span();
                        self.cursor.advance();
                    } else {
                        let at = self.peek_span();
                        end = self.synthetic(TokenKind::CloseCurly, at.range.offset);
                    }
                    break;
                }
            }
        }
        self.listener.handle_literal_set_or_map(
            count,
            left,
            const_keyword,
            end,
            has_set_entry.unwrap_or(false),
        );
        end
    }

    /// Parenthesized expression or record literal (reduced
    /// `parseParenthesizedExpressionOrRecordLiteral` :8307).
    pub(crate) fn parse_parenthesized_or_record_literal(
        &mut self,
        const_keyword: Option<TokenSpan>,
    ) -> TokenSpan {
        let begin = self.peek_span(); // `(`
        self.cursor.advance();
        self.listener
            .begin_parenthesized_expression_or_record_literal(begin);
        let mut count = 0usize;
        let mut was_record = false;
        let mut was_valid_record = false;
        let end;
        loop {
            let kind = self.peek_kind();
            if kind == TokenKind::Eof {
                end = self.peek_span();
                break;
            }
            if (count > 0 || was_record) && kind == TokenKind::CloseParen {
                end = self.peek_span();
                self.cursor.advance();
                break;
            }
            let mut colon: Option<TokenSpan> = None;
            if kind != TokenKind::OpenParen
                && (kind == TokenKind::Colon || self.cursor.peek_kind_ahead(1) == TokenKind::Colon)
            {
                // Named field `id:`; `(:` is recovery through the same path
                // (official :7685-7693 — one condition, no handleNoName on
                // the record-literal path): ensureIdentifier reports
                // missing_identifier on the `:` and synthesizes the name.
                was_record = true;
                was_valid_record = true;
                let name = self.ensure_identifier(IdentifierContext::NamedRecordFieldReference);
                colon = Some(name);
                self.cursor.advance();
            }
            if self.peek_kind() == TokenKind::CloseParen && count == 0 && !was_record {
                // `()`: empty record literal.
                was_record = true;
            }
            self.parse_constant_expression(ConstantPatternContext::None);
            count += 1;
            if self.peek_kind() == TokenKind::Comma {
                // A comma makes it a record.
                was_record = true;
                was_valid_record = true;
                self.cursor.advance();
            } else {
                let close = self.ensure_close_paren(begin);
                end = close;
                if was_record || colon.is_some() {
                    if let Some(c) = colon {
                        self.listener.handle_named_record_field(c);
                    } else {
                        self.listener.handle_positional_record_field(close);
                    }
                }
                break;
            }
        }
        if was_record {
            if count == 1 && !was_valid_record {
                self.report(msg::record_literal_one_positional_no_trailing_comma(), end);
            }
            self.listener.end_record_literal(end, count, const_keyword);
        } else {
            self.listener.handle_positional_record_field(end);
            self.listener.end_parenthesized_expression(end);
        }
        end
    }

    /// String literal with adjacent merging and interpolation (C21).
    pub(crate) fn parse_literal_string(&mut self) -> TokenSpan {
        let start = self.peek_span();
        let mut literal_count = 0usize;
        let mut end;
        loop {
            end = self.parse_single_literal_string();
            literal_count += 1;
            if self.peek_kind() != TokenKind::String {
                break;
            }
        }
        if literal_count > 1 {
            self.listener
                .handle_adjacent_string_literals(start, literal_count);
        }
        end
    }

    /// One string token, consuming `${...}` interpolation groups (the linear
    /// flow from the scanner: `String` + `${` group + `}` + `String`...).
    fn parse_single_literal_string(&mut self) -> TokenSpan {
        let begin = self.peek_span();
        self.listener.begin_literal_string(begin);
        self.cursor.advance();
        let mut interpolation_count = 0usize;
        loop {
            match self.peek_kind() {
                TokenKind::StringInterpolationExpression => {
                    let left_id = self.peek_id();
                    let left = self.peek_span();
                    self.cursor.advance();
                    self.parse_constant_expression(ConstantPatternContext::None);
                    // The scanner pairs `${` with its `}`.
                    let right = if let Some(id) = self.cursor.table_matching_end(left_id) {
                        self.skip_to_token(id);
                        let span = self.span_of(id);
                        self.cursor.advance();
                        span
                    } else {
                        let at = self.peek_span();
                        self.synthetic(TokenKind::CloseCurly, at.range.offset)
                    };
                    self.listener
                        .handle_interpolation_expression(left, Some(right));
                    interpolation_count += 1;
                }
                TokenKind::StringInterpolationIdentifier => {
                    let dollar = self.peek_span();
                    self.cursor.advance();
                    let id = self.ensure_identifier(IdentifierContext::Expression);
                    self.listener
                        .handle_interpolation_expression(dollar, Some(id));
                    interpolation_count += 1;
                }
                _ => break,
            }
            if self.peek_kind() == TokenKind::String {
                self.cursor.advance();
            } else {
                break;
            }
        }
        self.listener.end_literal_string(interpolation_count, begin);
        begin
    }

    /// Symbol body (official `parseLiteralSymbol` :9244-9268): a
    /// user-definable operator, `void`, or `(id .)* id`. Operator membership
    /// is `isUserDefinableOperator` — not the binary-level table — so `#is`
    /// / `#as` are rejected while `#~` / `#[]` are accepted.
    pub(crate) fn parse_symbol_rest(&mut self, hash: TokenSpan) -> TokenSpan {
        if is_user_definable_operator(self.peek_kind()) {
            let op = self.peek_span();
            self.cursor.advance();
            // Official emits `handleOperator`; the listener trait has no
            // operator event yet, so it surfaces as an identifier event.
            self.listener
                .handle_identifier(op, IdentifierContext::Expression);
            self.listener.end_literal_symbol(hash, 1);
            return op;
        }
        if self.cursor.opt_lexeme("void").is_some() {
            let void_tok = self.peek_span();
            self.listener.handle_symbol_void(hash);
            self.listener.end_literal_symbol(hash, 1);
            return void_tok;
        }
        let mut count = 1usize;
        let first = self.ensure_identifier(IdentifierContext::Expression);
        let mut end = first;
        while self.peek_kind() == TokenKind::Period {
            self.cursor.advance();
            end = self.ensure_identifier(IdentifierContext::ExpressionContinuation);
            count += 1;
        }
        self.listener.end_literal_symbol(hash, count);
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    /// Local listener capturing set/map literal shape, element counts, and
    /// error codes (the shared Collector does not log these events).
    #[derive(Default)]
    struct LiteralLog {
        errors: Vec<String>,
        identifiers: usize,
        int_literals: usize,
        map_entries: usize,
        set_or_map: Vec<bool>,
        no_name: usize,
        named_record_fields: usize,
    }

    impl crate::listener::ParserListener for LiteralLog {
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn handle_identifier(&mut self, _: TokenSpan, _: IdentifierContext) {
            self.identifiers += 1;
        }

        fn handle_no_name(&mut self, _: TokenSpan) {
            self.no_name += 1;
        }

        fn handle_named_record_field(&mut self, _: TokenSpan) {
            self.named_record_fields += 1;
        }

        fn handle_literal_int(&mut self, _: TokenSpan) {
            self.int_literals += 1;
        }

        fn handle_literal_map_entry(
            &mut self,
            _: TokenSpan,
            _: TokenSpan,
            _: Option<TokenSpan>,
            _: Option<TokenSpan>,
        ) {
            self.map_entries += 1;
        }

        fn handle_literal_set_or_map(
            &mut self,
            _: usize,
            _: TokenSpan,
            _: Option<TokenSpan>,
            _: TokenSpan,
            has_set_entry: bool,
        ) {
            self.set_or_map.push(has_set_entry);
        }
    }

    fn literal_parser(source: &'static str) -> crate::Parser<'static, 'static, LiteralLog> {
        crate::expr_const::test_support::parser(source, LiteralLog::default())
    }

    #[test]
    fn set_or_map_parses_each_entry_once() {
        // F4: the probe double-parse emitted every element twice; the
        // single pass parses `{1, 2, 3}` exactly once.
        let mut p = literal_parser("{1, 2, 3}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.int_literals, 3);
        assert_eq!(p.listener.map_entries, 0);
        assert_eq!(p.listener.set_or_map, vec![true]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn set_entry_wins_over_later_map_entry() {
        // F4: `hasSetEntry ??=` is first-wins (official :8596) — `{1, 2: 3}`
        // leans set even though a map entry follows.
        let mut p = literal_parser("{1, 2: 3}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.int_literals, 2);
        assert_eq!(p.listener.map_entries, 1);
        assert_eq!(p.listener.set_or_map, vec![true]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let mut p = literal_parser("{1: 2, 3}");
        let _ = p.parse_set_or_map_literal(None);
        assert_eq!(p.listener.set_or_map, vec![false]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }

    #[test]
    fn record_literal_colon_prefix_reports_missing_identifier() {
        // Official :7685-7693: `(:` takes the named-field path, where
        // ensureIdentifier(namedRecordFieldReference) reports
        // missing_identifier on the `:` and emits a synthetic name — the
        // record-literal path never emits handleNoName (that is record
        // *pattern* recovery, official :11568).
        let mut p = literal_parser("(: 1)");
        let _ = p.parse_parenthesized_or_record_literal(None);
        assert_eq!(p.listener.errors, vec!["missing_identifier".to_string()]);
        assert_eq!(p.listener.no_name, 0);
        assert_eq!(p.listener.identifiers, 1);
        assert_eq!(p.listener.named_record_fields, 1);

        // A well-formed named field parses without diagnostics.
        let mut p = literal_parser("(a: 1)");
        let _ = p.parse_parenthesized_or_record_literal(None);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
        assert_eq!(p.listener.no_name, 0);
        assert_eq!(p.listener.identifiers, 1);
        assert_eq!(p.listener.named_record_fields, 1);
    }

    #[test]
    fn list_literal_missing_comma_splits_elements() {
        // F10 (official :8549-8560): `[1 2]` splits into two elements with
        // exactly one expected-`,` report.
        let mut p = literal_parser("[1 2]");
        let _ = p.parse_list_literal(None);
        assert_eq!(p.listener.int_literals, 2);
        assert_eq!(p.listener.errors, vec!["expected_token".to_string()]);
    }

    /// Local listener capturing symbol membership and identifier events.
    #[derive(Default)]
    struct SymbolLog {
        errors: Vec<String>,
        identifiers: usize,
        dot_access: usize,
        symbol_ends: Vec<usize>,
    }

    impl crate::listener::ParserListener for SymbolLog {
        fn handle_recoverable_error(&mut self, message: Message, _: TokenSpan, _: TokenSpan) {
            self.errors.push(message.code.name.to_owned());
        }

        fn handle_identifier(&mut self, _: TokenSpan, _: IdentifierContext) {
            self.identifiers += 1;
        }

        fn handle_dot_access(&mut self, _: TokenSpan, _: TokenSpan, _: bool) {
            self.dot_access += 1;
        }

        fn end_literal_symbol(&mut self, _: TokenSpan, identifier_count: usize) {
            self.symbol_ends.push(identifier_count);
        }
    }

    fn symbol_parser(source: &'static str) -> crate::Parser<'static, 'static, SymbolLog> {
        crate::expr_const::test_support::parser(source, SymbolLog::default())
    }

    fn parse_symbol(source: &'static str) -> crate::Parser<'static, 'static, SymbolLog> {
        let mut p = symbol_parser(source);
        let hash = p.peek_span();
        p.cursor.advance();
        p.listener.begin_literal_symbol(hash);
        let end = p.parse_symbol_rest(hash);
        let _ = end;
        p
    }

    #[test]
    fn symbol_operator_membership_is_user_definable_operator() {
        // F11 (official :9244-9268): `~` is a member (count 1); keywords
        // `is`/`as` are not — they take the identifier path and report
        // missing_identifier exactly like the oracle.
        let p = parse_symbol("#~");
        assert_eq!(p.listener.symbol_ends, vec![1]);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);

        let p = parse_symbol("#is");
        assert_eq!(p.listener.symbol_ends, vec![1]);
        assert_eq!(p.listener.errors, vec!["missing_identifier".to_string()]);
    }

    #[test]
    fn symbol_identifier_chain_counts_dots() {
        let p = parse_symbol("#a.b");
        assert_eq!(p.listener.symbol_ends, vec![2]);
        // No dot-access event between symbol segments (official emits
        // nothing but the identifiers).
        assert_eq!(p.listener.dot_access, 0);
        assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
    }
}
