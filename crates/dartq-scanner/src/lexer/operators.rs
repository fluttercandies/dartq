//! Multi-character operator scanning (an aligned implementation of the official
//! same-name `tokenizeEquals` / `tokenizeGreaterThan` etc.; for `>>>` gating and
//! `<` group closing semantics see `scan_greater_than`).

#![allow(clippy::cast_possible_truncation)]

use crate::diag::msg;
use crate::lexer::{Flow, Lexer};
use crate::token::TokenKind;

impl Lexer<'_> {
    /// Three-way dispatch for `X` / `XX` / `X=`: `doubled` is the two-character
    /// form (`++`/`--`); operators without one (`*` `%` `^`) pass `base` as a placeholder.
    pub(crate) fn select_suffix(
        &mut self,
        start: usize,
        base: TokenKind,
        doubled: TokenKind,
        suffix_eq: TokenKind,
    ) -> Flow {
        self.bump();
        let second = if doubled == base {
            None
        } else {
            doubled_second(base)
        };
        match self.peek() {
            Some('=') => {
                self.bump();
                self.emit_fixed(suffix_eq, start);
            }
            Some(c) if Some(c) == second => {
                self.bump();
                self.emit_fixed(doubled, start);
            }
            _ => self.emit_fixed(base, start),
        }
        Flow::Next
    }

    // --- multi-character operators (aligned implementation of the official same-name tokenize* functions) ---

    /// `=` `==` `=>` `===` (`===` still emits a token + `unsupported_operator`;
    /// `=` triggers discardOpenLt, official tokenizeEquals).
    pub(crate) fn scan_equals(&mut self, start: usize) -> Flow {
        self.bump(); // '='
        if self.peek() == Some('=') {
            self.bump();
            if self.peek() == Some('=') {
                self.bump();
                self.emit_fixed(TokenKind::EqEqEq, start);
                self.diag(
                    crate::diag::UNSUPPORTED_OPERATOR,
                    || msg::unsupported_operator("==="),
                    start,
                    start + 1,
                );
            } else {
                self.emit_fixed(TokenKind::EqEq, start);
            }
        } else if self.peek() == Some('>') {
            self.bump();
            self.emit_fixed(TokenKind::Function, start);
        } else {
            self.emit_fixed(TokenKind::Eq, start);
        }
        self.discard_open_lt();
        Flow::Next
    }

    /// `!` `!=` `!==` (official tokenizeExclamation; no `!.` single-token form).
    pub(crate) fn scan_bang(&mut self, start: usize) -> Flow {
        self.bump(); // '!'
        if self.peek() == Some('=') {
            self.bump();
            if self.peek() == Some('=') {
                self.bump();
                self.emit_fixed(TokenKind::BangEqEq, start);
                self.diag(
                    crate::diag::UNSUPPORTED_OPERATOR,
                    || msg::unsupported_operator("!=="),
                    start,
                    start + 1,
                );
            } else {
                self.emit_fixed(TokenKind::BangEq, start);
            }
        } else {
            self.emit_fixed(TokenKind::Bang, start);
        }
        Flow::Next
    }

    /// `<` `<=` `<<` `<<=`; `<` opens a group (official tokenizeLessThan).
    pub(crate) fn scan_less_than(&mut self, start: usize) -> Flow {
        self.bump(); // '<'
        match self.peek() {
            Some('=') => {
                self.bump();
                self.emit_fixed(TokenKind::LtEq, start);
            }
            Some('<') => {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    self.emit_fixed(TokenKind::LtLtEq, start);
                } else {
                    self.emit_fixed(TokenKind::LtLt, start);
                }
            }
            _ => self.begin_group(TokenKind::Lt, start),
        }
        Flow::Next
    }

    /// `>` `>=` `>>` `>>=` `>>>` `>>>=` (`>>>` is gated; `>>` is always a single
    /// token and never split, official tokenizeGreaterThan :1479; closing `<`
    /// groups never errors).
    pub(crate) fn scan_greater_than(&mut self, start: usize) -> Flow {
        self.bump(); // '>'
        if self.peek() == Some('=') {
            self.bump();
            self.emit_fixed(TokenKind::GtEq, start);
        } else if self.peek() == Some('>') {
            self.bump();
            if self.peek() == Some('=') {
                self.bump();
                self.emit_fixed(TokenKind::GtGtEq, start);
            } else if self.config.enable_triple_shift && self.peek() == Some('>') {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    self.emit_fixed(TokenKind::GtGtGtEq, start);
                } else {
                    self.emit_fixed(TokenKind::GtGtGt, start);
                }
            } else {
                self.emit_fixed(TokenKind::GtGt, start);
            }
        } else {
            self.emit_fixed(TokenKind::Gt, start);
        }
        // Close `<` groups: Gt closes one, GtGt two, GtGtGt three (official appendGt*).
        // Only the **outermost** closed `<` records endGroup (official: "in T<U<V>>,
        // the '>>' token closes the outer '<', the inner '<' is left without endGroup").
        let closes = match self.last_token().map(|t| t.kind) {
            Some(TokenKind::Gt) => 1,
            Some(TokenKind::GtGt) => 2,
            Some(TokenKind::GtGtGt) => 3,
            _ => 0,
        };
        let gt_idx = self.last_index();
        for i in 0..closes {
            match self.groups.last() {
                Some(&(top, TokenKind::Lt)) => {
                    self.groups.pop();
                    if i + 1 == closes {
                        self.record_end(top, gt_idx);
                    }
                }
                _ => break,
            }
        }
        Flow::Next
    }

    /// `&` `&&` `&=` (official tokenizeAmpersand; `&&=` never forms a single
    /// token — `LAZY_ASSIGNMENT_ENABLED` is a hard `false`, `abstract_scanner.dart:53`).
    pub(crate) fn scan_ampersand(&mut self, start: usize) -> Flow {
        self.bump(); // '&'
        match self.peek() {
            Some('&') => {
                self.bump();
                // The `&&=` branch (official :1354) is dead code: the `=` is
                // rescanned by the main loop as a plain `=`.
                self.emit_fixed(TokenKind::AmpersandAmpersand, start);
            }
            Some('=') => {
                self.bump();
                self.emit_fixed(TokenKind::AmpersandEq, start);
            }
            _ => self.emit_fixed(TokenKind::Ampersand, start),
        }
        Flow::Next
    }

    /// `|` `||` `|=` (official tokenizeBar; `||=` never forms a single token —
    /// `LAZY_ASSIGNMENT_ENABLED` is a hard `false`, `abstract_scanner.dart:53`).
    pub(crate) fn scan_bar(&mut self, start: usize) -> Flow {
        self.bump(); // '|'
        match self.peek() {
            Some('|') => {
                self.bump();
                // The `||=` branch (official :1375) is dead code: the `=` is
                // rescanned by the main loop as a plain `=`.
                self.emit_fixed(TokenKind::BarBar, start);
            }
            Some('=') => {
                self.bump();
                self.emit_fixed(TokenKind::BarEq, start);
            }
            _ => self.emit_fixed(TokenKind::Bar, start),
        }
        Flow::Next
    }

    /// `?` `?.` `?..` `??` `??=` (official tokenizeQuestion).
    pub(crate) fn scan_question(&mut self, start: usize) -> Flow {
        self.bump(); // '?'
        match self.peek() {
            Some('?') => {
                self.bump();
                if self.peek() == Some('=') {
                    self.bump();
                    self.emit_fixed(TokenKind::QuestionQuestionEq, start);
                } else {
                    self.emit_fixed(TokenKind::QuestionQuestion, start);
                }
            }
            Some('.') => {
                self.bump();
                if self.peek() == Some('.') {
                    self.bump();
                    self.emit_fixed(TokenKind::QuestionPeriodPeriod, start);
                } else {
                    self.emit_fixed(TokenKind::QuestionPeriod, start);
                }
            }
            _ => self.emit_fixed(TokenKind::Question, start),
        }
        Flow::Next
    }
}

/// Second character of two-character operators (`+`→`+`, `-`→`-`; the rest have no doubled form).
fn doubled_second(base: TokenKind) -> Option<char> {
    match base {
        TokenKind::Plus => Some('+'),
        TokenKind::Minus => Some('-'),
        _ => None,
    }
}
