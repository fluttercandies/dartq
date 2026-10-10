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
//! Escape sequences are validated against the official unescape rules
//! (`.third/dart-sdk` `pkg/_fe_analyzer_shared/lib/src/parser/quote.dart`,
//! `unescapeCodeUnits`): `\x` wants exactly two hex digits, `\u` a
//! `{1-6 hex}` bracket or four bare hex digits, values above 0x10FFFF are
//! invalid, and a part cannot end with a lone backslash. Unescaping happens
//! per string part there and returns at the first error, so this scanner
//! also stops validating after one malformed escape per segment (the
//! `escape_err` frame flag); the missing-`}` report for `\u{xxxxxxx}` is the
//! official exception that keeps validating.
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
use dartq_diagnostics::{
    CodeInfo, INVALID_CODE_POINT, INVALID_HEX_ESCAPE, INVALID_UNICODE_ESCAPE_STARTED,
    INVALID_UNICODE_ESCAPE_U_BRACKET, INVALID_UNICODE_ESCAPE_U_NO_BRACKET,
    INVALID_UNICODE_ESCAPE_U_STARTED, UNTERMINATED_STRING_LITERAL,
};

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
    /// A malformed escape was already reported in the current segment:
    /// official unescape runs per string part and returns at the first
    /// error, so the rest of the segment passes through unvalidated. Reset
    /// wherever `segment_start` restarts.
    escape_err: bool,
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
                        frame.escape_err = false;
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
                        while suspended.pop().is_some() {
                            self.unterminated();
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
                escape_err: false,
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
            escape_err: false,
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
                self.unterminated();
                return StringBody::Done;
            };
            match c {
                _ if c == f.quote => {
                    self.bump();
                    self.emit_fixed(TokenKind::String, f.segment_start);
                    return StringBody::Done;
                }
                '\\' => {
                    let backslash = self.pos;
                    self.bump();
                    // The escaped character is checked for LF/CR after the
                    // backslash advance (official abstract_scanner.dart
                    // tokenizeSingleLineString): an escaped newline still
                    // terminates the string. The content keeps the backslash
                    // (the official last part carries it; the synthetic
                    // closing quote makes unescape see it), so it reports
                    // invalidEscapeStarted — unless this segment already
                    // returned from an escape error.
                    if matches!(self.peek(), None | Some('\n' | '\r')) {
                        if !f.escape_err {
                            self.escape_started(f, backslash);
                        }
                        self.unterminated();
                        return StringBody::Done;
                    }
                    self.escape(f, backslash);
                }
                '$' => {
                    if self.scan_dollar(f) {
                        return StringBody::Interpolation;
                    }
                }
                '\n' | '\r' => {
                    self.unterminated();
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
                    self.unterminated();
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
                self.unterminated();
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
                let backslash = self.pos;
                self.bump();
                if self.peek().is_none() {
                    // EOF right after the backslash: the part's content ends
                    // with a lone `\` (corpus string2.dart:21) — reported
                    // unless this segment already had an escape error.
                    if !f.escape_err {
                        self.escape_started(f, backslash);
                    }
                    self.unterminated();
                    return StringBody::Done;
                }
                self.escape(f, backslash);
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
        f.escape_err = false;
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

    /// Validates and consumes the escape whose backslash sits at `backslash`
    /// (already consumed; the lookahead is the designator character).
    /// Mirrors the official unescape decision order per string part
    /// (quote.dart `unescapeCodeUnits`): `\x` with exactly two hex digits,
    /// `\u` with a `{1-6 hex}` bracket or four bare hex digits, code points
    /// capped at 0x10FFFF, and every other escaped character passing through.
    fn escape(&mut self, f: &mut StringFrame, backslash: usize) {
        if f.escape_err {
            // This segment already reported a malformed escape (official
            // unescapeCodeUnits returns at the first error): the rest of the
            // segment passes through unvalidated.
            self.bump();
            return;
        }
        match self.peek() {
            Some('x') => self.hex_escape(f, backslash),
            Some('u') => self.unicode_escape(f, backslash),
            // Any other escaped character passes through unchanged.
            _ => {
                self.bump();
            }
        }
    }

    /// `\x` with exactly two hex digits (quote.dart `invalidHexEscape`).
    /// Official order: the presence of both digit code units is checked
    /// first (a short part reports with its range reaching the content end),
    /// then digit validity left to right (the bad digit is excluded from the
    /// range and left in the stream as ordinary content).
    fn hex_escape(&mut self, f: &mut StringFrame, backslash: usize) {
        self.bump(); // 'x'
        if let Err(boundary) = self.digit_units(f, self.pos, 2) {
            self.escape_error(
                f,
                INVALID_HEX_ESCAPE,
                msg::INVALID_HEX_ESCAPE,
                backslash,
                boundary,
            );
            return;
        }
        // Both digit units are present; each must be a hex digit. A multi-unit
        // (astral) character contributes a non-hex high unit at its position.
        let d1 = self
            .peek()
            .expect("presence walk guarantees a digit character");
        if d1.len_utf16() == 2 || hex_value(d1).is_none() {
            self.escape_error(
                f,
                INVALID_HEX_ESCAPE,
                msg::INVALID_HEX_ESCAPE,
                backslash,
                self.pos,
            );
            return;
        }
        let at2 = self.pos + d1.len_utf8();
        let d2 = self
            .peek_char(at2)
            .expect("presence walk guarantees a second digit unit");
        if d2.len_utf16() == 2 || hex_value(d2).is_none() {
            self.escape_error(
                f,
                INVALID_HEX_ESCAPE,
                msg::INVALID_HEX_ESCAPE,
                backslash,
                at2,
            );
            return;
        }
        self.bump();
        self.bump();
    }

    /// `\u` escape (quote.dart `invalidUnicodeEscapeU*` / `invalidCodePoint`):
    /// either `u{` + 1-6 hex digits + `}` or four bare hex digits, with the
    /// bracket value capped at 0x10FFFF.
    fn unicode_escape(&mut self, f: &mut StringFrame, backslash: usize) {
        self.bump(); // 'u'
        if self.part_end(f, self.pos, self.peek()) {
            // The segment's content ends right after `\u`.
            self.escape_error(
                f,
                INVALID_UNICODE_ESCAPE_U_STARTED,
                msg::INVALID_UNICODE_ESCAPE_U_STARTED,
                backslash,
                self.pos,
            );
            return;
        }
        if self.peek() != Some('{') {
            self.unicode_escape_digits(f, backslash);
            return;
        }
        self.bump(); // '{'
        let mut code: u32 = 0;
        let mut end_bracket = None;
        let mut seventh = self.pos;
        for j in 0..7 {
            if self.part_end(f, self.pos, self.peek()) {
                // Content end inside the bracket digits (j == 0 covers `\u{`
                // with the part ending right after the brace).
                self.escape_error(
                    f,
                    INVALID_UNICODE_ESCAPE_U_BRACKET,
                    msg::INVALID_UNICODE_ESCAPE_U_BRACKET,
                    backslash,
                    self.pos,
                );
                return;
            }
            let c = self.peek().expect("part_end guarantees a character");
            let at = self.pos;
            if j != 0 && c == '}' {
                self.bump();
                end_bracket = Some(at);
                break;
            }
            if j == 6 {
                // Seventh character: the official loop breaks without adding
                // it to the value, reports the missing `}` — and, alone among
                // the errors, keeps validating the rest of the segment.
                self.bump();
                seventh = at;
                break;
            }
            let Some(d) = hex_value(c) else {
                // Bad digit; the official range includes the offending
                // character, which stays in the stream as ordinary content.
                self.escape_error(
                    f,
                    INVALID_UNICODE_ESCAPE_U_BRACKET,
                    msg::INVALID_UNICODE_ESCAPE_U_BRACKET,
                    backslash,
                    at + c.len_utf8(),
                );
                return;
            };
            self.bump();
            code = (code << 4) | d;
        }
        if end_bracket.is_none() {
            self.diag(
                INVALID_UNICODE_ESCAPE_U_BRACKET,
                || msg::INVALID_UNICODE_ESCAPE_U_BRACKET.to_owned(),
                backslash,
                seventh,
            );
        }
        if code > 0x10_FFFF {
            // The range ends before the `}` (or at the seventh character).
            self.escape_error(
                f,
                INVALID_CODE_POINT,
                msg::INVALID_CODE_POINT,
                backslash,
                end_bracket.unwrap_or(seventh),
            );
        }
    }

    /// The bare `\u` form: exactly four hex-digit code units (quote.dart
    /// `invalidUnicodeEscapeUNoBracket`). Official order: presence of all
    /// four units first, then digit validity left to right. Four digits cap
    /// at 0xFFFF, so no code point check applies.
    fn unicode_escape_digits(&mut self, f: &mut StringFrame, backslash: usize) {
        let end = match self.digit_units(f, self.pos, 4) {
            Ok(end) => end,
            Err(boundary) => {
                self.escape_error(
                    f,
                    INVALID_UNICODE_ESCAPE_U_NO_BRACKET,
                    msg::INVALID_UNICODE_ESCAPE_U_NO_BRACKET,
                    backslash,
                    boundary,
                );
                return;
            }
        };
        let mut at = self.pos;
        while at < end {
            let c = self
                .peek_char(at)
                .expect("presence walk guarantees the digit units");
            if c.len_utf16() == 2 || hex_value(c).is_none() {
                self.escape_error(
                    f,
                    INVALID_UNICODE_ESCAPE_U_NO_BRACKET,
                    msg::INVALID_UNICODE_ESCAPE_U_NO_BRACKET,
                    backslash,
                    at,
                );
                return;
            }
            at += c.len_utf8();
        }
        for _ in 0..4 {
            self.bump();
        }
    }

    /// `invalidEscapeStarted` (quote.dart): a part's content ends with a lone
    /// backslash, which can't stand alone.
    fn escape_started(&mut self, f: &mut StringFrame, backslash: usize) {
        self.escape_error(
            f,
            INVALID_UNICODE_ESCAPE_STARTED,
            msg::INVALID_ESCAPE_STARTED,
            backslash,
            backslash + 1,
        );
    }

    /// Reports one malformed escape and disables further escape validation in
    /// the segment (official unescapeCodeUnits returns at the first error).
    /// The range starts at the backslash; `end` is the byte offset of the
    /// official range boundary (its UTF-16 width matches the official
    /// code-unit length after conversion).
    fn escape_error(
        &mut self,
        f: &mut StringFrame,
        code: CodeInfo,
        message: &'static str,
        backslash: usize,
        end: usize,
    ) {
        f.escape_err = true;
        self.diag(code, || message.to_owned(), backslash, end);
    }

    /// Whether the character `c` at byte offset `at` ends the current
    /// segment's content for escape validation — what official unescape
    /// would not see as a code unit of this part: EOF, the `$` that starts
    /// an interpolation, the closing quote of a single-line string, the
    /// closing triple quote of a multiline string (one or two quotes are
    /// ordinary content there), and the line terminator ending a
    /// single-line string.
    fn part_end(&self, f: &StringFrame, at: usize, c: Option<char>) -> bool {
        let Some(c) = c else {
            return true;
        };
        if c == '$' {
            return true;
        }
        if c == f.quote {
            if !f.triple {
                return true;
            }
            let q = f.quote as u8; // ' or " — ASCII
            let b = self.src.as_bytes();
            return b.get(at + 1) == Some(&q) && b.get(at + 2) == Some(&q);
        }
        matches!(c, '\n' | '\r') && !f.triple
    }

    /// Fixed digit-count presence walk (official `codeUnits.length` checks
    /// against the part end): walks up to `units` UTF-16 code units from
    /// `at` without consuming. `Ok` returns the offset past the units; `Err`
    /// returns the byte offset where the content ended (the official
    /// too-few-digits range boundary). An astral character contributes two
    /// units, matching the official code-unit counting.
    fn digit_units(&self, f: &StringFrame, mut at: usize, units: u32) -> Result<usize, usize> {
        let mut left = units;
        while left > 0 {
            let Some(c) = self.peek_char(at) else {
                return Err(at);
            };
            if self.part_end(f, at, Some(c)) {
                return Err(at);
            }
            at += c.len_utf8();
            left = left.saturating_sub(c.len_utf16() as u32);
        }
        Ok(at)
    }

    /// Unterminated string: a synthetic empty STRING segment (pinned where
    /// scanning stopped) + diagnostic (pinned to the last UTF-16 unit:
    /// official `UnterminatedString` + analyzer-side `endOffset-1`, `length 1`).
    /// The registry-true message carries no opener/close placeholder.
    fn unterminated(&mut self) {
        let stop = self.pos;
        self.emit_synthetic(TokenKind::String, stop, 0);
        self.diag_pin_last_unit(
            UNTERMINATED_STRING_LITERAL,
            || msg::UNTERMINATED_STRING.to_owned(),
            stop,
        );
    }
}

/// Hex digit value (`characters.dart` `hexDigitValue`); `None` for anything
/// else, including the surrogate halves of an astral character.
fn hex_value(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(c as u32 - '0' as u32),
        'a'..='f' => Some(c as u32 - 'a' as u32 + 10),
        'A'..='F' => Some(c as u32 - 'A' as u32 + 10),
        _ => None,
    }
}
