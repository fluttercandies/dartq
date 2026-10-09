//! String scanning sub-state machine: single/double quotes, raw, triple-quoted
//! multiline, escapes, and interpolation (an aligned implementation of the
//! official `tokenizeString` / `tokenizeSingleLineString` /
//! `tokenizeMultiLineString` / `tokenizeStringInterpolation`).
//!
//! For the token sequence contract see "Interpolated string token sequence
//! shape" in the crate docs. An unterminated string produces a synthetic empty
//! STRING segment (pinned where scanning stopped) +
//! `unterminated_string_literal`; the diagnostic range pins to the last
//! UTF-16 unit (the analyzer's translation rule for `ErrorToken`,
//! `endOffset - 1, length 1` — the low surrogate when the final character
//! is astral).

use crate::diag::msg;
use crate::keyword::{Keyword, keyword_from_str};
use crate::lexer::{Flow, Lexer};
use crate::token::TokenKind;
use dartq_diagnostics::UNTERMINATED_STRING_LITERAL;

impl Lexer<'_> {
    /// `next` is the opening quote character; `quote_start` is the string start (including `r` for raw).
    pub(crate) fn scan_string(&mut self, quote_start: usize, raw: bool) -> Flow {
        let quote = self.bump().unwrap_or('"');
        // Triple-quote detection: `''` in a row → multiline (official tokenizeString).
        if self.peek() == Some(quote) && self.peek2() == Some(quote) {
            self.bump();
            self.bump();
            self.scan_multiline(quote_start, quote, raw);
            return Flow::Next;
        }
        if self.peek() == Some(quote) {
            // Empty string `''`.
            self.bump();
            self.emit_fixed(TokenKind::String, quote_start);
            return Flow::Next;
        }
        if raw {
            self.scan_single_line_raw(quote_start, quote);
        } else {
            self.scan_single_line(quote_start, quote);
        }
        Flow::Next
    }

    /// Single-line non-raw: escapes and interpolation; line termination means unterminated (official tokenizeSingleLineString).
    fn scan_single_line(&mut self, quote_start: usize, quote: char) {
        let mut segment_start = quote_start;
        loop {
            let Some(c) = self.peek() else {
                self.unterminated(quote, false, false);
                return;
            };
            match c {
                _ if c == quote => {
                    self.bump();
                    self.emit_fixed(TokenKind::String, segment_start);
                    return;
                }
                '\\' => {
                    self.bump();
                    self.bump(); // the escaped character (EOF reports unterminated on the next iteration)
                }
                '$' => {
                    // The prefix segment ends **before** `$` (official corpus
                    // scanner_test.dart:1034-1051: `"Hello "` [0..7) with `${`
                    // at 7); the SII/SIE token starts at `$`.
                    self.emit_fixed(TokenKind::String, segment_start);
                    self.bump();
                    self.scan_interpolation();
                    segment_start = self.pos;
                }
                '\n' | '\r' => {
                    self.unterminated(quote, false, false);
                    return;
                }
                _ => {
                    let q = quote as u8;
                    self.skip_bytes_until(|b| b == q || matches!(b, b'\\' | b'$' | b'\n' | b'\r'));
                }
            }
        }
    }

    /// Single-line raw: no escapes, no interpolation (official tokenizeSingleLineRawString).
    fn scan_single_line_raw(&mut self, quote_start: usize, quote: char) {
        loop {
            match self.peek() {
                None | Some('\n' | '\r') => {
                    self.unterminated(quote, true, false);
                    return;
                }
                Some(c) if c == quote => {
                    self.bump();
                    self.emit_fixed(TokenKind::String, quote_start);
                    return;
                }
                Some(_) => {
                    let q = quote as u8;
                    self.skip_bytes_until(|b| b == q || matches!(b, b'\n' | b'\r'));
                }
            }
        }
    }

    /// Multiline (official tokenizeMultiLineString / tokenizeMultiLineRawString).
    fn scan_multiline(&mut self, quote_start: usize, quote: char, raw: bool) {
        let mut segment_start = quote_start;
        loop {
            let Some(c) = self.peek() else {
                self.unterminated(quote, raw, true);
                return;
            };
            if !raw && c == '$' {
                // Same contract as the single-line form: the prefix segment
                // excludes `$`.
                self.emit_fixed(TokenKind::String, segment_start);
                self.bump();
                self.scan_interpolation();
                segment_start = self.pos;
                continue;
            }
            if c == quote {
                // `'''` closes; fewer than three continue as content (official isomorphic loop).
                self.bump();
                if self.peek() == Some(quote) {
                    self.bump();
                    if self.peek() == Some(quote) {
                        self.bump();
                        self.emit_fixed(TokenKind::String, segment_start);
                        return;
                    }
                }
                continue;
            }
            if !raw && c == '\\' {
                self.bump();
                if self.peek().is_none() {
                    self.unterminated(quote, raw, true);
                    return;
                }
                self.bump();
                continue;
            }
            if raw {
                let q = quote as u8;
                self.skip_bytes_until(|b| b == q);
            } else {
                let q = quote as u8;
                self.skip_bytes_until(|b| b == q || matches!(b, b'$' | b'\\'));
            }
        }
    }

    /// Interpolation dispatch: `next` is the first character after `$` (official tokenizeStringInterpolation).
    fn scan_interpolation(&mut self) {
        match self.peek() {
            Some('{') => {
                // The SIE token lexeme is `${` (starts at `$`, matching the official overlapping shape).
                let dollar_start = self.pos - 1; // `$` is a single byte
                self.bump(); // '{'
                self.begin_group(TokenKind::StringInterpolationExpression, dollar_start);
                loop {
                    match self.code_step() {
                        // CloseInterp: `}` closed the `${`, return to the string body (official $STX equivalent);
                        // Eof: an unclosed `${` gets its synthetic close token from EOF recovery.
                        Flow::CloseInterp | Flow::Eof => return,
                        Flow::Next => {}
                    }
                }
            }
            Some('a'..='z' | 'A'..='Z' | '_') => {
                // The `$` token is followed by a keyword-or-identifier (keywords allowed, same as official).
                let dollar_start = self.pos - 1; // `$` is a single byte
                self.emit_fixed(TokenKind::StringInterpolationIdentifier, dollar_start);
                let ident_start = self.pos;
                while matches!(self.peek(), Some('a'..='z' | 'A'..='Z' | '0'..='9' | '_')) {
                    self.bump();
                }
                let lexeme = &self.src[ident_start..self.pos];
                let kind = match keyword_from_str(lexeme) {
                    Some(Keyword::Augment) if !self.config.enable_augmentations => {
                        TokenKind::Identifier
                    }
                    Some(kw) => TokenKind::Keyword(kw),
                    None => TokenKind::Identifier,
                };
                // Type parameters cannot contain `this`: discard an open `<`
                // group (official appendKeywordToken, abstract_scanner.dart:376-381).
                if matches!(kind, TokenKind::Keyword(Keyword::This)) {
                    self.discard_open_lt();
                }
                self.emit(kind, ident_start, self.pos - ident_start);
            }
            _ => {
                // Non-identifier after `$`: synthetic empty Identifier + missing_identifier
                // (official unexpectedDollarInString → analyzer missing_identifier).
                let dollar_start = self.pos - 1;
                self.emit_fixed(TokenKind::StringInterpolationIdentifier, dollar_start);
                // The pin sits at the character after `$` (official
                // synthetic-identifier scanOffset, tokenizeInterpolatedIdentifier);
                // at EOF it backs off one unit (translateErrorToken's
                // `_isAtEnd` decrement) onto the 1-byte `$`. The byte cap is
                // applied BEFORE the boundary snap: a `.min` after `pin_back`
                // could cut a boundary-safe offset into a multi-byte
                // character, and `TokenTable::lexeme` panics on
                // non-boundary offsets.
                let at = self.pin_back(self.pos.min(self.src.len().saturating_sub(1)));
                self.emit_synthetic(TokenKind::Identifier, at, 0);
                self.diag(
                    dartq_diagnostics::MISSING_IDENTIFIER,
                    msg::MISSING_IDENTIFIER.to_owned(),
                    at,
                    at + 1,
                );
            }
        }
    }

    /// Unterminated string: a synthetic empty STRING segment (pinned where
    /// scanning stopped) + diagnostic (pinned to the last UTF-16 unit:
    /// official `UnterminatedString` + analyzer-side `endOffset-1`, `length 1`).
    fn unterminated(&mut self, quote: char, raw: bool, multiline: bool) {
        let stop = self.pos;
        self.emit_synthetic(TokenKind::String, stop, 0);
        let close = if multiline {
            quote_triple(quote)
        } else {
            quote.to_string()
        };
        let open = if raw {
            format!("r{close}")
        } else {
            close.clone()
        };
        self.diag_pin_last_unit(
            UNTERMINATED_STRING_LITERAL,
            msg::unterminated_string(&open, &close),
            stop,
        );
    }
}

fn quote_triple(quote: char) -> String {
    quote.to_string().repeat(3)
}
