//! Number scanning sub-state machine: an aligned implementation of the official
//! `tokenizeNumber` / `tokenizeHex` / `tokenizeFractionPart` (diagnostics for
//! every illegal digit-separator position; synthetic recovery for `0x` missing
//! digits and `1e` missing exponent).
//!
//! The official scanner uses dedicated `*_WITH_SEPARATORS` kinds for numbers
//! with valid separators — those exist only to serve parser-side error text,
//! which the dartq parser does not consume, so we do not split kinds.
//! Diagnostic pinning aligns with the analyzer translation: separator errors
//! pin to the **number start** (the official `UnterminatedToken` charOffset),
//! while `missing_hex_digit` / `missing_digit` pin to the **number's last
//! character** (`endOffset - 1`).

#![allow(clippy::cast_possible_truncation)]

use crate::diag::{MISSING_DIGIT, MISSING_HEX_DIGIT, UNEXPECTED_SEPARATOR_IN_NUMBER, msg};
use crate::lexer::{Flow, Lexer};
use crate::token::TokenKind;

impl Lexer<'_> {
    /// `next` is the first digit character (`1-9` or `0`).
    pub(crate) fn scan_number(&mut self) -> Flow {
        let start = self.pos;
        if self.peek() == Some('0') && matches!(self.peek2(), Some('x' | 'X')) {
            self.scan_hex(start);
            return Flow::Next;
        }
        self.scan_int_body(start);
        Flow::Next
    }

    /// Starts with `.`: `.5` fraction / `..` / `...` / `...?` / single `.`
    /// (official tokenizeDotsOrNumber).
    pub(crate) fn scan_dots(&mut self, start: usize) -> Flow {
        self.bump(); // '.'
        if matches!(self.peek(), Some('0'..='9')) {
            self.scan_fraction(start);
        } else if self.peek() == Some('.') {
            self.bump();
            if self.peek() == Some('.') {
                self.bump();
                if self.peek() == Some('?') {
                    self.bump();
                    self.emit_fixed(TokenKind::PeriodPeriodPeriodQuestion, start);
                } else {
                    self.emit_fixed(TokenKind::PeriodPeriodPeriod, start);
                }
            } else {
                self.emit_fixed(TokenKind::PeriodPeriod, start);
            }
        } else {
            self.emit_fixed(TokenKind::Period, start);
        }
        Flow::Next
    }

    /// Integer body: digits and `_`; `e` enters fraction/exponent, `.` depends
    /// on what follows (`1.` = INT + PERIOD, official semantics); a letter
    /// directly after digits (`1x`) does not error — scanned as two tokens.
    fn scan_int_body(&mut self, start: usize) {
        let mut prev_sep = false;
        loop {
            match self.peek() {
                Some('0'..='9') => {
                    prev_sep = false;
                    self.bump();
                }
                Some('_') => {
                    prev_sep = true;
                    self.bump();
                }
                Some('e' | 'E') => {
                    if prev_sep {
                        self.separator_diag(start);
                    }
                    self.scan_fraction(start);
                    return;
                }
                Some('.') => {
                    if prev_sep {
                        self.separator_diag(start);
                    }
                    if matches!(self.peek2(), Some('0'..='9')) {
                        self.bump(); // '.'
                        self.scan_fraction(start);
                    } else {
                        self.finish(start, TokenKind::Int, prev_sep);
                    }
                    return;
                }
                _ => {
                    self.finish(start, TokenKind::Int, prev_sep);
                    return;
                }
            }
        }
    }

    /// Finishes the number token; trailing separators are reported here (the official end-of-number checkpoint).
    fn finish(&mut self, start: usize, kind: TokenKind, prev_sep: bool) {
        if prev_sep {
            self.separator_diag(start);
        }
        self.emit_fixed(kind, start);
    }

    /// `0x` hexadecimal (official tokenizeHex).
    fn scan_hex(&mut self, start: usize) {
        self.bump(); // '0'
        self.bump(); // 'x'
        let mut has_digits = false;
        let mut prev_sep = false;
        loop {
            match self.peek() {
                Some('0'..='9' | 'a'..='f' | 'A'..='F') => {
                    has_digits = true;
                    prev_sep = false;
                    self.bump();
                }
                Some('_') => {
                    if !has_digits {
                        self.separator_diag(start);
                    }
                    prev_sep = true;
                    self.bump();
                }
                _ => {
                    if !has_digits {
                        // `0x` with no digits: synthetic `0` + missing_hex_digit
                        // (pinned to the last character: analyzer endOffset-1 = `x`).
                        let at = self.pin_back(self.pos - 1).max(start);
                        self.diag(
                            MISSING_HEX_DIGIT,
                            || msg::MISSING_HEX_DIGIT.to_owned(),
                            at,
                            at + 1,
                        );
                        self.emit_synthetic(TokenKind::Hexadecimal, start, 1);
                        return;
                    }
                    self.finish(start, TokenKind::Hexadecimal, prev_sep);
                    return;
                }
            }
        }
    }

    /// Fraction/exponent body (on entry `next` is a digit or e/E; official tokenizeFractionPart).
    fn scan_fraction(&mut self, start: usize) {
        let mut prev_sep = false;
        loop {
            match self.peek() {
                Some('0'..='9') => {
                    prev_sep = false;
                    self.bump();
                }
                Some('_') => {
                    prev_sep = true;
                    self.bump();
                }
                Some('e' | 'E') => {
                    if prev_sep {
                        self.separator_diag(start);
                    }
                    prev_sep = false;
                    self.bump();
                    // Each `_` before the exponent is reported individually (official while loop).
                    while self.peek() == Some('_') {
                        self.separator_diag(start);
                        prev_sep = true;
                        self.bump();
                    }
                    if matches!(self.peek(), Some('+' | '-')) {
                        prev_sep = false;
                        self.bump();
                    }
                    let mut has_exp_digits = false;
                    loop {
                        match self.peek() {
                            Some('0'..='9') => {
                                has_exp_digits = true;
                                prev_sep = false;
                                self.bump();
                            }
                            Some('_') => {
                                if !has_exp_digits {
                                    self.separator_diag(start);
                                }
                                prev_sep = true;
                                self.bump();
                            }
                            _ => {
                                if !has_exp_digits {
                                    // `1e` with no exponent: synthetic `0` DOUBLE + missing_digit
                                    // (pinned to the last character: analyzer endOffset-1).
                                    let at = self.pin_back(self.pos - 1).max(start);
                                    self.diag(
                                        MISSING_DIGIT,
                                        || msg::MISSING_DIGIT.to_owned(),
                                        at,
                                        at + 1,
                                    );
                                    self.emit_synthetic(TokenKind::Double, start, 1);
                                    return;
                                }
                                self.finish(start, TokenKind::Double, prev_sep);
                                return;
                            }
                        }
                    }
                }
                _ => {
                    self.finish(start, TokenKind::Double, prev_sep);
                    return;
                }
            }
        }
    }

    /// `unexpected_separator_in_number`: pinned to the number start (official charOffset semantics).
    fn separator_diag(&mut self, start: usize) {
        self.diag(
            UNEXPECTED_SEPARATOR_IN_NUMBER,
            || msg::UNEXPECTED_SEPARATOR_IN_NUMBER.to_owned(),
            start,
            start + 1,
        );
    }
}
