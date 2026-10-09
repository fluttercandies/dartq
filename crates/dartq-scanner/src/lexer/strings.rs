//! String scanning sub-state machine: single/double quotes, raw, triple-quoted
//! multiline, escapes, and interpolation (an aligned implementation of the
//! official `tokenizeString` / `tokenizeSingleLineString` /
//! `tokenizeMultiLineString` / `tokenizeStringInterpolation`).
//!
//! Interpolation nesting is **iterative**: `${` pushes the current body onto a
//! frame stack and the driver scans the expression in code mode; `}`
//! (`Flow::CloseInterp`) pops back into the body. The official scanner is flat
//! the same way (`tokenizeInterpolatedExpression` rides its grouping stack and
//! reenters `bigSwitch`); the recursive alternative (body → interpolation →
//! `code_step` → body) would grow the native call stack with the nesting
//! level, so deeply nested input could overflow it — here the frames live on
//! the heap and the driver is a flat loop.
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

/// One string body suspended by an open `${`: everything needed to resume body
/// scanning once the interpolation's `}` arrives.
#[derive(Clone, Copy)]
struct StringFrame {
    quote: char,
    raw: bool,
    triple: bool,
    /// Source offset where the current STRING segment began (the opening
    /// quote for the first segment; just past the last `}` / `$id` after).
    segment_start: usize,
}

/// Outcome of scanning one string body (content mode).
enum StringBody {
    /// The body ended — closing quote consumed with the final segment
    /// emitted, or unterminated recovery emitted. Control falls back to the
    /// enclosing expression's code mode, or the string is complete when
    /// nothing is suspended.
    Done,
    /// `${` was consumed and the SII begin group emitted: the driver scans
    /// code mode next; `Flow::CloseInterp` resumes the body.
    Interpolation,
}

impl Lexer<'_> {
    /// Scans one string literal to completion, including every interpolation
    /// nested inside it. `quote_start` is the string start (including `r` for
    /// raw); returns `Flow::Eof` when the scan ended at EOF with interpolation
    /// frames still open (after their recovery), so `run` closes the groups.
    pub(crate) fn scan_string(&mut self, quote_start: usize, raw: bool) -> Flow {
        let Some(mut frame) = self.open_string(quote_start, raw) else {
            return Flow::Next; // empty string `''`
        };
        // Bodies suspended by an open `${`. The stack mirrors the open SII
        // begin groups 1:1, LIFO: a push coincides with every `begin_group(SII)`
        // in `scan_dollar`, a pop with the `}` that closes one — so
        // `Flow::CloseInterp` always resumes the body that opened the group.
        // Empty until the first `${`: a plain string never allocates.
        let mut suspended: Vec<StringFrame> = Vec::new();
        // Each outer iteration scans one string body (content mode); each
        // inner iteration scans interpolation expression code (code mode)
        // until a body re-enters.
        loop {
            match self.scan_body(&mut frame) {
                StringBody::Done => {
                    // Fall back to the enclosing expression; with nothing
                    // suspended the string is complete.
                    if suspended.is_empty() {
                        return Flow::Next;
                    }
                }
                StringBody::Interpolation => suspended.push(frame),
            }
            loop {
                match self.code_step() {
                    Flow::Next => {}
                    // A nested literal inside the expression (an empty string
                    // `''` stays in code mode).
                    Flow::BeginString { start, raw } => {
                        if let Some(nested) = self.open_string(start, raw) {
                            frame = nested;
                            break;
                        }
                    }
                    Flow::CloseInterp => {
                        frame = suspended.pop().expect("SII close without a suspended body");
                        frame.segment_start = self.pos;
                        break;
                    }
                    Flow::Eof => {
                        // Unwind innermost-first: every open level is a frame
                        // on the stack (pushed at its `${`, not yet resumed),
                        // and each emits its own unterminated recovery — the
                        // order the recursive shape produced as its Eof
                        // propagated out through one body per level. Then
                        // report Eof so `run` closes the still-open SII
                        // groups (synthetic `}` innermost first).
                        while let Some(f) = suspended.pop() {
                            self.unterminated(f.quote, f.raw, f.triple);
                        }
                        return Flow::Eof;
                    }
                }
            }
        }
    }

    /// Consumes the opening quote(s); `quote_start` is the string start
    /// (including `r` for raw). Returns the frame for the body scan, or `None`
    /// for the empty string `''` (already emitted).
    fn open_string(&mut self, quote_start: usize, raw: bool) -> Option<StringFrame> {
        let quote = self.bump().unwrap_or('"');
        // Triple-quote detection: `''` in a row → multiline (official tokenizeString).
        if self.peek() == Some(quote) && self.peek2() == Some(quote) {
            self.bump();
            self.bump();
            return Some(StringFrame {
                quote,
                raw,
                triple: true,
                segment_start: quote_start,
            });
        }
        if self.peek() == Some(quote) {
            // Empty string `''`.
            self.bump();
            self.emit_fixed(TokenKind::String, quote_start);
            return None;
        }
        Some(StringFrame {
            quote,
            raw,
            triple: false,
            segment_start: quote_start,
        })
    }

    /// Body (content mode) dispatch on the frame's string form.
    fn scan_body(&mut self, f: &mut StringFrame) -> StringBody {
        match (f.raw, f.triple) {
            (false, false) => self.scan_single_line(f),
            (true, false) => self.scan_single_line_raw(f),
            (_, true) => self.scan_multiline(f),
        }
    }

    /// Single-line non-raw: escapes and interpolation; line termination means unterminated (official tokenizeSingleLineString).
    fn scan_single_line(&mut self, f: &mut StringFrame) -> StringBody {
        loop {
            let Some(c) = self.peek() else {
                self.unterminated(f.quote, false, false);
                return StringBody::Done;
            };
            match c {
                _ if c == f.quote => {
                    self.bump();
                    self.emit_fixed(TokenKind::String, f.segment_start);
                    return StringBody::Done;
                }
                '\\' => {
                    self.bump();
                    self.bump(); // the escaped character (EOF reports unterminated on the next iteration)
                }
                '$' => {
                    if self.scan_dollar(f) {
                        return StringBody::Interpolation;
                    }
                }
                '\n' | '\r' => {
                    self.unterminated(f.quote, false, false);
                    return StringBody::Done;
                }
                _ => {
                    let q = f.quote as u8;
                    self.skip_bytes_until(|b| b == q || matches!(b, b'\\' | b'$' | b'\n' | b'\r'));
                }
            }
        }
    }

    /// Single-line raw: no escapes, no interpolation (official tokenizeSingleLineRawString).
    fn scan_single_line_raw(&mut self, f: &mut StringFrame) -> StringBody {
        loop {
            match self.peek() {
                None | Some('\n' | '\r') => {
                    self.unterminated(f.quote, true, false);
                    return StringBody::Done;
                }
                Some(c) if c == f.quote => {
                    self.bump();
                    self.emit_fixed(TokenKind::String, f.segment_start);
                    return StringBody::Done;
                }
                Some(_) => {
                    let q = f.quote as u8;
                    self.skip_bytes_until(|b| b == q || matches!(b, b'\n' | b'\r'));
                }
            }
        }
    }

    /// Multiline (official tokenizeMultiLineString / tokenizeMultiLineRawString).
    fn scan_multiline(&mut self, f: &mut StringFrame) -> StringBody {
        loop {
            let Some(c) = self.peek() else {
                self.unterminated(f.quote, f.raw, true);
                return StringBody::Done;
            };
            if !f.raw && c == '$' {
                if self.scan_dollar(f) {
                    return StringBody::Interpolation;
                }
                continue;
            }
            if c == f.quote {
                // `'''` closes; fewer than three continue as content (official isomorphic loop).
                self.bump();
                if self.peek() == Some(f.quote) {
                    self.bump();
                    if self.peek() == Some(f.quote) {
                        self.bump();
                        self.emit_fixed(TokenKind::String, f.segment_start);
                        return StringBody::Done;
                    }
                }
                continue;
            }
            if !f.raw && c == '\\' {
                self.bump();
                if self.peek().is_none() {
                    self.unterminated(f.quote, f.raw, true);
                    return StringBody::Done;
                }
                self.bump();
                continue;
            }
            if f.raw {
                let q = f.quote as u8;
                self.skip_bytes_until(|b| b == q);
            } else {
                let q = f.quote as u8;
                self.skip_bytes_until(|b| b == q || matches!(b, b'$' | b'\\'));
            }
        }
    }

    /// `$` inside a non-raw string body: emits the prefix STRING segment —
    /// which ends **before** `$` (official corpus scanner_test.dart:1034-1051:
    /// `"Hello "` [0..7) with `${` at 7) — then either opens a `${`
    /// interpolation (returns `true`; the driver scans code mode until
    /// `Flow::CloseInterp`) or scans the identifier form inline (returns
    /// `false`; the body continues with a fresh segment).
    fn scan_dollar(&mut self, f: &mut StringFrame) -> bool {
        self.emit_fixed(TokenKind::String, f.segment_start);
        self.bump(); // '$'
        if self.peek() == Some('{') {
            // The SIE token lexeme is `${` (starts at `$`, matching the official overlapping shape).
            let dollar_start = self.pos - 1; // `$` is a single byte
            self.bump(); // '{'
            self.begin_group(TokenKind::StringInterpolationExpression, dollar_start);
            return true;
        }
        self.scan_interpolated_identifier();
        f.segment_start = self.pos;
        false
    }

    /// `$` followed by a non-`{` (official tokenizeInterpolatedIdentifier):
    /// a keyword-or-identifier, or the missing-identifier recovery.
    fn scan_interpolated_identifier(&mut self) {
        match self.peek() {
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
                    || msg::MISSING_IDENTIFIER.to_owned(),
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
            || msg::unterminated_string(&open, &close),
            stop,
        );
    }
}

fn quote_triple(quote: char) -> String {
    quote.to_string().repeat(3)
}
