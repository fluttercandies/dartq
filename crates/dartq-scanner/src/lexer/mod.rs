//! Scan state machine core: single pass, zero backtracking. The structure maps
//! one-to-one onto the official `AbstractScanner.bigSwitch`; the number/string
//! sub-state machines live in `numbers.rs` / `strings.rs`.
//!
//! Truncating casts are allowed: source length ≤ `u32::MAX` is a dartq-source
//! engine contract (`LineIndex::new` panics directly on oversized sources), so
//! offsets in this layer can always be safely narrowed.

#![allow(clippy::cast_possible_truncation)]

mod curly_recovery;
mod numbers;
mod operators;
mod strings;

use crate::diag::{ScanDiag, msg};
use crate::keyword::{Keyword, keyword_from_str};
use crate::token::{Token, TokenKind, TokenTable};
use crate::{ScanConfig, ScanResult};
use dartq_diagnostics::{CodeInfo, EXPECTED_TOKEN};
use dartq_language::LanguageVersion;
use dartq_source::{FileId, LineIndex, SourceRange};

/// Flow-control signals of `code_step`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// Consumed a chunk of input; continue the main loop.
    Next,
    /// Reached EOF (`self.next == None`).
    Eof,
    /// `}` closed a `${`: return to string body scanning (equivalent of the official `$STX` signal).
    CloseInterp,
    /// A string literal starts at `start` (`raw`: the `r` prefix is already
    /// consumed). The caller enters string-body scanning instead of calling
    /// `scan_string` right here — the recursive shape (body → interpolation →
    /// `code_step` → body) grew the native call stack with the interpolation
    /// nesting level and overflowed it on deeply nested input.
    BeginString { start: usize, raw: bool },
}

/// Per-file diagnostics cap, parser parity (`MAX_DIAGNOSTICS` in
/// dartq-parser; the same stop-recording-but-keep-going semantics as the
/// parser's bail-out). The scanner runs BEFORE the parser's own fuse, so it
/// bounds itself: past the cap, recording stops but scanning continues to
/// EOF — the token stream shape never depends on the cap.
const MAX_DIAGNOSTICS: usize = 10_000;

pub(crate) struct Lexer<'s> {
    src: &'s str,
    config: ScanConfig,
    file: FileId,
    /// Lookahead register: the character at `self.pos`; `None` = EOF.
    next: Option<char>,
    pos: usize,
    tokens: Vec<Token>,
    /// begin token index → close token index (grows in parallel with tokens).
    end_of: Vec<u32>,
    /// Open-group stack (token indices; `<` is pushed as well — see lib.rs for the silent-discard semantics).
    groups: Vec<u32>,
    /// Capped at [`MAX_DIAGNOSTICS`] so adversarial input (an illegal
    /// character per byte, each with a fresh `format!` message) cannot grow
    /// this without bound before the parser's own fuse applies.
    diags: Vec<ScanDiag>,
    /// UTF-16 count invariant: always equals `utf16(self.pos)` (maintained
    /// incrementally in bump; emit walks back by the lexeme to get the token
    /// start value — avoiding a second full-source cursor pass).
    utf16_value: u32,
    version_override: Option<LanguageVersion>,
    /// UTF-16 span of the comment lexeme behind `version_override` (set and
    /// cleared together with it; last match wins, same block).
    version_override_span: Option<SourceRange>,
    /// Two-pass curly recovery (`None` on the first pass): byte offset of the
    /// `{`/`${` the re-scan blames — see `curly_recovery`. Error-path-only
    /// bookkeeping; never set on a clean file.
    curly_recovery: Option<u32>,
}

impl<'s> Lexer<'s> {
    pub(crate) fn new(src: &'s str, config: ScanConfig, file: FileId) -> Self {
        // The decode faces (engine `set_content` / CLI `read_source`) strip a
        // leading BOM, so the scanner always starts at the true file origin.
        let mut lex = Self {
            src,
            config,
            file,
            next: None,
            pos: 0,
            tokens: Vec::with_capacity(src.len() / 4 + 1),
            end_of: Vec::new(),
            groups: Vec::new(),
            diags: Vec::new(),
            utf16_value: 0,
            version_override: None,
            version_override_span: None,
            curly_recovery: None,
        };
        lex.next = lex.peek_char(0);
        lex
    }

    pub(crate) fn run(mut self) -> ScanResult {
        let (result, curly_begin) = self.scan_once();
        // Official scan/scanString (scanner.dart:90-108, 126-144): when the
        // first pass errored while ending with exactly one unclosed `{`, a
        // misaligned `{`/`${` is located in the token stream and the source
        // is re-scanned once with that offset (the "user is typing" shape).
        // Both gates are error-path-only: a clean file pays nothing beyond
        // the O(1) empty-diagnostics check.
        let recovery = curly_begin
            .filter(|_| !result.diagnostics.is_empty())
            .and_then(|begin| curly_recovery::recovery_offset(self.src, &result.tokens, begin));
        match recovery {
            None => result,
            Some(offset) => {
                let mut second = Lexer::new(self.src, self.config, self.file);
                second.curly_recovery = Some(offset as u32);
                second.scan_once().0
            }
        }
    }

    fn scan_once(&mut self) -> (ScanResult, Option<u32>) {
        let source_len = self.src.len();
        self.scan_header();
        loop {
            match self.code_step() {
                Flow::Next => {}
                // A top-level string literal: `scan_string` runs it to
                // completion, including every interpolation nested inside it.
                Flow::BeginString { start, raw } => {
                    if self.scan_string(start, raw) == Flow::Eof {
                        break;
                    }
                }
                // CloseInterp is only consumed inside string interpolation scanning; it never reaches the top level.
                // Eof ends the main loop.
                Flow::Eof | Flow::CloseInterp => break,
            }
        }
        let curly_begin = self.close_unclosed_groups();
        self.emit(TokenKind::Eof, source_len, 0);
        let tokens = std::mem::take(&mut self.tokens);
        let end_of = std::mem::take(&mut self.end_of);
        let diags = std::mem::take(&mut self.diags);
        let result = ScanResult {
            tokens: TokenTable::new(tokens, end_of),
            diagnostics: crate::diag::finish(diags, self.src, self.file),
            language_version_override: self.version_override,
            language_version_override_span: self.version_override_span,
            line_index: LineIndex::new(self.src),
        };
        (result, curly_begin)
    }

    // --- basic advancement ---

    /// ASCII fast path: the vast majority of source bytes are < 0x80; reading
    /// bytes directly avoids UTF-8 decoding overhead.
    #[inline]
    fn peek_char(&self, at: usize) -> Option<char> {
        let b = *self.src.as_bytes().get(at)?;
        if b < 0x80 {
            Some(b as char)
        } else {
            self.src[at..].chars().next()
        }
    }

    /// Consumes and returns the current lookahead character (advances the register).
    #[inline]
    fn bump(&mut self) -> Option<char> {
        let current = self.next;
        if let Some(c) = current {
            self.pos += c.len_utf8();
            self.utf16_value += c.len_utf16() as u32;
            self.next = self.peek_char(self.pos);
        }
        current
    }

    /// The current lookahead character (not consumed).
    #[inline]
    fn peek(&self) -> Option<char> {
        self.next
    }

    /// ASCII byte sweep: consumes runs of bytes satisfying `f` (hot paths such
    /// as whitespace/identifiers skip per-character UTF-8 decoding); syncs the
    /// lookahead register at the end.
    #[inline]
    fn bump_ascii_while(&mut self, f: impl Fn(u8) -> bool) {
        let bytes = self.src.as_bytes();
        while let Some(&b) = bytes.get(self.pos) {
            if !f(b) {
                break;
            }
            self.pos += 1;
            self.utf16_value += 1;
        }
        self.next = self.peek_char(self.pos);
    }

    /// Byte fast-forward: advances up to the first byte satisfying `stop` (the
    /// stop byte is not consumed; stops at EOF). ASCII counts 1 UTF-16 unit;
    /// multi-byte sequences are counted precisely per character.
    fn skip_bytes_until(&mut self, stop: impl Fn(u8) -> bool) {
        let bytes = self.src.as_bytes();
        let mut p = self.pos;
        while let Some(&b) = bytes.get(p) {
            if stop(b) {
                break;
            }
            if b < 0x80 {
                p += 1;
                self.utf16_value += 1;
            } else {
                let ch = self.src[p..].chars().next().unwrap();
                p += ch.len_utf8();
                self.utf16_value += ch.len_utf16() as u32;
            }
        }
        if p != self.pos {
            self.pos = p;
            self.next = self.peek_char(p);
        }
    }

    /// The second character after the lookahead register (does not advance the register).
    fn peek2(&self) -> Option<char> {
        let after = self.pos + self.next.map_or(0, char::len_utf8);
        self.peek_char(after)
    }

    // --- token / diagnostic emission ---

    fn emit(&mut self, kind: TokenKind, start: usize, len: usize) -> u32 {
        // Invariant utf16_value == utf16(self.pos); walk back by the lexeme's
        // UTF-16 length (ASCII lexemes — the vast majority — use the byte length).
        let lexeme = &self.src[start..start + len];
        let utf16_len = if lexeme.is_ascii() {
            len as u32
        } else {
            lexeme.chars().map(|c| c.len_utf16() as u32).sum()
        };
        let utf16 = self.utf16_value - utf16_len;
        let idx = self.tokens.len() as u32;
        self.tokens
            .push(Token::new(kind, start as u32, len as u32, utf16));
        self.end_of.push(crate::token::NO_END);
        idx
    }

    /// Synthetic token during recovery (`is_synthetic` = true; the source slice
    /// is the recovered lexeme). The UTF-16 offset is derived by walking back
    /// over `src[start..self.pos]`, **not** over the lexeme: synthetic recovery
    /// tokens can start at the number start while the cursor already sits past
    /// the missing digits (`0x` → synthetic `0`, `1e` → synthetic `0`; official
    /// `SyntheticStringToken` offset = number start), so a lexeme-based walk
    /// back would drift by the skipped suffix. Zero-length synthetics at the
    /// current position walk back over an empty slice (unchanged value).
    fn emit_synthetic(&mut self, kind: TokenKind, start: usize, len: usize) {
        let skipped = &self.src[start..self.pos];
        let utf16 = if skipped.is_ascii() {
            skipped.len() as u32
        } else {
            skipped.chars().map(|c| c.len_utf16() as u32).sum()
        };
        let utf16 = self.utf16_value - utf16;
        let token = Token::new(kind, start as u32, len as u32, utf16).synthetic();
        self.tokens.push(token);
        self.end_of.push(crate::token::NO_END);
    }

    /// Token ending at the current lookahead position (lexeme = source `start..self.pos`).
    fn emit_fixed(&mut self, kind: TokenKind, start: usize) {
        let len = self.pos - start;
        self.emit(kind, start, len);
    }

    fn begin_group(&mut self, kind: TokenKind, start: usize) {
        if !matches!(kind, TokenKind::Lt | TokenKind::OpenParen) {
            self.discard_open_lt();
        }
        let idx = self.emit(kind, start, self.pos - start);
        self.groups.push(idx);
    }

    fn discard_open_lt(&mut self) {
        while let Some(&top) = self.groups.last() {
            if self.tokens[top as usize].kind == TokenKind::Lt {
                self.groups.pop();
            } else {
                break;
            }
        }
    }

    /// Recording past [`MAX_DIAGNOSTICS`] is dropped (cap semantics above).
    /// The message is a lazy closure, run only after the cap check: capped-out
    /// input pays no message formatting or allocation.
    fn diag(&mut self, code: CodeInfo, message: impl FnOnce() -> String, start: usize, end: usize) {
        if self.diags.len() >= MAX_DIAGNOSTICS {
            return;
        }
        self.diags.push(ScanDiag {
            code,
            message: message(),
            start: start as u32,
            end: end as u32,
            pin_last_unit: false,
        });
    }

    /// Diagnostic pinned to the single UTF-16 unit ending at `stop` (analyzer
    /// `endOffset - 1, length 1`, `translate_error_token.dart`): the unit is
    /// the character's low surrogate when it is astral, which no byte range
    /// can address, so the backoff is applied after UTF-16 conversion.
    fn diag_pin_last_unit(
        &mut self,
        code: CodeInfo,
        message: impl FnOnce() -> String,
        stop: usize,
    ) {
        if self.diags.len() >= MAX_DIAGNOSTICS {
            return;
        }
        self.diags.push(ScanDiag {
            code,
            message: message(),
            start: stop as u32,
            end: stop as u32,
            pin_last_unit: true,
        });
    }

    /// Byte-safe version of pinning a diagnostic to the last character (analyzer
    /// `endOffset - 1`; multi-byte characters walk back to a char boundary to
    /// avoid slicing panics).
    fn pin_back(&self, byte: usize) -> usize {
        let mut b = byte.min(self.src.len());
        while b > 0 && !self.src.is_char_boundary(b) {
            b -= 1;
        }
        b
    }

    // --- header: script tag and the @dart version comment window ---

    fn scan_header(&mut self) {
        // script tag: only when `#!` opens the file (official scanOffset==0).
        // scan_header runs before any content is consumed, so the current
        // position here is always the file origin.
        if self.peek() == Some('#') && self.peek2() == Some('!') {
            let start = self.pos;
            self.bump();
            self.bump();
            while !matches!(self.peek(), None | Some('\n' | '\r')) {
                self.bump();
            }
            self.emit(TokenKind::ScriptTag, start, self.pos - start);
        }
        // Comment window: consecutive whitespace/comments emit no tokens; the window stays open (official tail-unchanged semantics).
        loop {
            match self.peek() {
                Some(' ' | '\t' | '\r' | '\n') => {
                    self.bump();
                }
                Some('/') if self.peek2() == Some('/') => self.scan_line_comment_header(),
                Some('/') if self.peek2() == Some('*') => {
                    self.scan_block_comment();
                }
                _ => return,
            }
        }
    }

    /// `//` line comment inside the header window: recognizes `// @dart = x.y`
    /// first (strict syntax, official
    /// `tokenizeLanguageVersionOrSingleLineComment`); any step failing to match
    /// treats the whole line as a plain comment.
    fn scan_line_comment_header(&mut self) {
        let comment_start = self.pos;
        self.bump(); // '/'
        self.bump(); // '/'
        if self.peek() == Some('/') {
            self.skip_to_line_end(); // `///` doc comments are not version comments
            return;
        }
        self.skip_spaces_inline();
        if !self.consume_str("@dart") {
            self.skip_to_line_end();
            return;
        }
        self.skip_spaces_inline();
        if self.peek() != Some('=') {
            self.skip_to_line_end();
            return;
        }
        self.bump();
        self.skip_spaces_inline();
        let Some(version) = self.consume_version_number() else {
            self.skip_to_line_end();
            return;
        };
        self.skip_spaces_inline();
        if !matches!(self.peek(), None | Some('\n' | '\r')) {
            self.skip_to_line_end();
            return;
        }
        // Full-line validation passed; the **last** valid version comment wins.
        // Oracle path: the `dart analyze` scanner's `_languageVersionChanged`
        // fires on every languageVersionChanged and unconditionally reassigns
        // `_overrideVersion` (:206) and `scanner.configuration` (:222)
        // (analyzer-8.4.1 scanner.dart:195-222); the CFE first-wins path
        // (registerExplicitLanguageVersion, source_compilation_unit.dart:486)
        // is a different pipeline, not our parity basis.
        self.version_override = Some(version);
        // The span mirrors the official LanguageVersionToken: the whole
        // comment lexeme from the `//` through the last character before
        // the line terminator (trailing spaces included, CR excluded —
        // createLanguageVersionToken at the comment start,
        // abstract_scanner.dart:1909; the analyzer's above-latest warning
        // reuses it verbatim, scanner.dart:124
        // `.atOffset(versionToken.offset, versionToken.length)`). The
        // cursor sits at the terminator/EOF, so `utf16_value` is the
        // comment's UTF-16 end.
        let lexeme = &self.src[comment_start..self.pos];
        let utf16_len = if lexeme.is_ascii() {
            lexeme.len() as u32
        } else {
            lexeme.chars().map(|c| c.len_utf16() as u32).sum()
        };
        self.version_override_span = Some(SourceRange {
            offset: self.utf16_value - utf16_len,
            length: utf16_len,
        });
    }

    /// Reads `digit+ '.' digit+` (not committed; the caller writes it after full-line validation).
    fn consume_version_number(&mut self) -> Option<LanguageVersion> {
        let major = self.consume_digits()?;
        if self.peek() != Some('.') {
            return None;
        }
        self.bump();
        let minor = self.consume_digits()?;
        Some(LanguageVersion::new(major, minor))
    }

    fn consume_str(&mut self, pat: &str) -> bool {
        for expected in pat.chars() {
            if self.peek() != Some(expected) {
                return false;
            }
            self.bump();
        }
        true
    }

    fn consume_digits(&mut self) -> Option<u32> {
        let mut value: u32 = 0;
        let mut any = false;
        while let Some(c @ '0'..='9') = self.peek() {
            any = true;
            value = value
                .saturating_mul(10)
                .saturating_add(u32::from(c as u8 - b'0'));
            self.bump();
        }
        any.then_some(value)
    }

    fn skip_spaces_inline(&mut self) {
        while self.peek() == Some(' ') {
            self.bump();
        }
    }

    fn skip_to_line_end(&mut self) {
        while !matches!(self.peek(), None | Some('\n' | '\r')) {
            self.bump();
        }
    }

    // --- comments (main stream emits no tokens; block comments nest, official tokenizeMultiLineComment) ---

    fn scan_slash_or_comment(&mut self, start: usize) -> Flow {
        self.bump(); // '/'
        match self.peek() {
            Some('*') => {
                self.bump(); // '*' (the `/` is already consumed above)
                self.scan_block_comment_body();
                Flow::Next
            }
            Some('/') => {
                self.skip_to_line_end();
                Flow::Next
            }
            Some('=') => {
                self.bump();
                self.emit_fixed(TokenKind::SlashEq, start);
                Flow::Next
            }
            _ => {
                self.emit_fixed(TokenKind::Slash, start);
                Flow::Next
            }
        }
    }

    /// Nested block comments; an unterminated one emits `unterminated_multi_line_comment` (pinned to the last character).
    fn scan_block_comment(&mut self) {
        self.bump(); // '/'
        self.bump(); // '*'
        self.scan_block_comment_body();
    }

    /// The nesting loop proper (both `/*` characters already consumed —
    /// entered either from the header window or after `/*` in the main stream;
    /// the previous single-entry form re-bumped and swallowed one content
    /// character, mis-scanning `/**/`).
    fn scan_block_comment_body(&mut self) {
        let mut nesting = 1usize;
        loop {
            match self.peek() {
                None => {
                    // Pinned to the last UTF-16 unit (analyzer `endOffset - 1,
                    // length 1`, translate_error_token.dart:48-57): the low
                    // surrogate when the final character is astral.
                    let stop = self.pos;
                    self.diag_pin_last_unit(
                        crate::diag::UNTERMINATED_MULTI_LINE_COMMENT,
                        || msg::UNTERMINATED_COMMENT.to_owned(),
                        stop,
                    );
                    return;
                }
                Some('*') => {
                    self.bump();
                    if self.peek() == Some('/') {
                        self.bump();
                        nesting -= 1;
                        if nesting == 0 {
                            return;
                        }
                    }
                }
                Some('/') => {
                    self.bump();
                    if self.peek() == Some('*') {
                        self.bump();
                        nesting += 1;
                    }
                }
                Some(_) => {
                    self.skip_bytes_until(|b| matches!(b, b'*' | b'/'));
                }
            }
        }
    }

    // --- one step of the main loop (official bigSwitch) ---

    fn code_step(&mut self) -> Flow {
        self.bump_ascii_while(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'));
        let Some(c) = self.peek() else {
            return Flow::Eof;
        };
        let start = self.pos;
        match c {
            'a'..='z' | 'A'..='Z' | '_' | '$' => self.scan_identifier_or_raw(start),
            '0'..='9' => self.scan_number(),
            '"' | '\'' => Flow::BeginString { start, raw: false },
            '(' => {
                self.bump();
                self.begin_group(TokenKind::OpenParen, start);
                Flow::Next
            }
            ')' => {
                self.bump();
                self.end_group(TokenKind::CloseParen, start)
            }
            '[' => self.scan_open_square(start),
            ']' => {
                self.bump();
                self.end_group(TokenKind::CloseSquare, start)
            }
            '{' => {
                self.bump();
                self.begin_group(TokenKind::OpenCurly, start);
                Flow::Next
            }
            '}' => {
                self.bump();
                self.end_group(TokenKind::CloseCurly, start)
            }
            ';' => {
                self.bump();
                self.emit_fixed(TokenKind::Semicolon, start);
                self.discard_open_lt();
                Flow::Next
            }
            ',' => self.select1(TokenKind::Comma, start),
            ':' => self.select1(TokenKind::Colon, start),
            '@' => self.select1(TokenKind::At, start),
            '`' => self.select1(TokenKind::Backping, start),
            '\\' => self.select1(TokenKind::Backslash, start),
            '#' => self.select1(TokenKind::Hash, start),
            '.' => self.scan_dots(start),
            '=' => self.scan_equals(start),
            '!' => self.scan_bang(start),
            '<' => self.scan_less_than(start),
            '>' => self.scan_greater_than(start),
            '&' => self.scan_ampersand(start),
            '|' => self.scan_bar(start),
            '+' => self.select_suffix(
                start,
                TokenKind::Plus,
                TokenKind::PlusPlus,
                TokenKind::PlusEq,
            ),
            '-' => self.select_suffix(
                start,
                TokenKind::Minus,
                TokenKind::MinusMinus,
                TokenKind::MinusEq,
            ),
            '*' => self.select_suffix(start, TokenKind::Star, TokenKind::Star, TokenKind::StarEq),
            '%' => self.select_suffix(
                start,
                TokenKind::Percent,
                TokenKind::Percent,
                TokenKind::PercentEq,
            ),
            '/' => self.scan_slash_or_comment(start),
            '^' => self.select_suffix(
                start,
                TokenKind::Caret,
                TokenKind::Caret,
                TokenKind::CaretEq,
            ),
            '~' => {
                self.bump();
                if self.peek() == Some('/') {
                    self.bump();
                    if self.peek() == Some('=') {
                        self.bump();
                        self.emit_fixed(TokenKind::TildeSlashEq, start);
                    } else {
                        self.emit_fixed(TokenKind::TildeSlash, start);
                    }
                } else {
                    self.emit_fixed(TokenKind::Tilde, start);
                }
                Flow::Next
            }
            '?' => self.scan_question(start),
            _ => self.unexpected_char(c, start),
        }
    }

    fn select1(&mut self, kind: TokenKind, start: usize) -> Flow {
        self.bump();
        self.emit_fixed(kind, start);
        Flow::Next
    }

    // --- illegal characters (official unexpected/buildUnexpectedCharacterToken :2470) ---

    fn unexpected_char(&mut self, c: char, start: usize) -> Flow {
        if c == '\u{FFFD}' {
            // U+FFFD: skip silently. The official scanner has an
            // EncodingErrorToken path (error_token.dart:25-26 →
            // translate_error_token.dart:33-35), but the pinned oracle emits
            // ZERO diagnostics for both reachable U+FFFD classes: a raw
            // undecodable byte (the read layer tolerates it) and a literal
            // U+FFFD in valid UTF-8 — measured against
            // `dart analyze --format=machine` (docs/facts
            // corpus-expansion-plan.md §7). Measured-cull doctrine governs:
            // dartq emits nothing here. The character is still consumed
            // (never merged into an identifier), preserving termination.
            self.bump();
            return Flow::Next;
        }
        if crate::diag::is_unicode_space(c) {
            // Non-ASCII whitespace: skip (official advanceAfterError).
            self.diag(
                crate::diag::ILLEGAL_CHARACTER,
                || msg::illegal_character(c),
                start,
                start + c.len_utf8(),
            );
            self.bump();
            return Flow::Next;
        }
        if c < '\u{1f}' {
            // ASCII control characters 0x00-0x1E: skip (official
            // AsciiControlCharacterToken, error_token.dart:17 `character < 0x1f`).
            // 0x1F and 0x7F (DEL) do NOT take this arm: the official default
            // arm classifies them as NonAsciiIdentifierToken, i.e. they take
            // the merge path below.
            self.diag(
                crate::diag::ILLEGAL_CHARACTER,
                || msg::illegal_character(c),
                start,
                start + c.len_utf8(),
            );
            self.bump();
            return Flow::Next;
        }
        if c.len_utf16() == 2 {
            // Astral character: the official UTF-16 scanners see each code
            // unit separately, so each surrogate hits the default arm and is
            // reported on its own (oracle `var x = 1;\n😀`: illegal_character
            // @11 len1 char 55357 and @12 len1 char 56832). Each unit is
            // addressed by pinning to its end (`pin_last_unit`): the high
            // surrogate's unit ends one byte in, the low surrogate's at the
            // character end.
            let cp = c as u32;
            let high = 0xD800 + ((cp - 0x10000) >> 10);
            let low = 0xDC00 + ((cp - 0x10000) & 0x3FF);
            self.diag_pin_last_unit(
                crate::diag::ILLEGAL_CHARACTER,
                || msg::illegal_character_cp(high),
                start + 1,
            );
            self.diag_pin_last_unit(
                crate::diag::ILLEGAL_CHARACTER,
                || msg::illegal_character_cp(low),
                start + c.len_utf8(),
            );
        } else {
            self.diag(
                crate::diag::ILLEGAL_CHARACTER,
                || msg::illegal_character(c),
                start,
                start + c.len_utf8(),
            );
        }
        // Non-ASCII identifier-like characters (and DEL): recover by merging into the
        // adjacent identifier (official NonAsciiIdentifierToken recovery shape —
        // the preceding adjacent IDENTIFIER is folded into the new token, then
        // identifier characters keep being consumed).
        let mut ident_start = start;
        if let Some(last) = self.tokens.last()
            && last.kind == TokenKind::Identifier
            && last.byte_offset + last.byte_len == start as u32
        {
            ident_start = last.byte_offset as usize;
            self.tokens.pop();
            self.end_of.pop();
        }
        self.bump(); // the illegal character itself becomes part of the token content
        while matches!(
            self.peek(),
            Some('a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '$')
        ) {
            self.bump();
        }
        self.emit(TokenKind::Identifier, ident_start, self.pos - ident_start);
        Flow::Next
    }

    // --- identifier / raw string prefix (official tokenizeKeywordOrIdentifier) ---

    fn scan_identifier_or_raw(&mut self, start: usize) -> Flow {
        if self.peek() == Some('r') && matches!(self.peek2(), Some('"' | '\'')) {
            self.bump(); // 'r'
            return Flow::BeginString { start, raw: true };
        }
        while matches!(
            self.peek(),
            Some('a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '$')
        ) {
            self.bump();
        }
        let lexeme = &self.src[start..self.pos];
        let kind = match keyword_from_str(lexeme) {
            Some(Keyword::Augment) if !self.config.enable_augmentations => TokenKind::Identifier,
            Some(kw) => TokenKind::Keyword(kw),
            None => TokenKind::Identifier,
        };
        // Type parameters cannot contain `this`: discard an open `<` group
        // (official appendKeywordToken, abstract_scanner.dart:376-381).
        if matches!(kind, TokenKind::Keyword(Keyword::This)) {
            self.discard_open_lt();
        }
        self.emit(kind, start, self.pos - start);
        Flow::Next
    }

    // --- `[` / `[]` / `[]=` (official tokenizeOpenSquareBracket) ---

    fn scan_open_square(&mut self, start: usize) -> Flow {
        self.bump(); // '['
        if self.peek() == Some(']') {
            self.bump();
            if self.peek() == Some('=') {
                self.bump();
                self.emit_fixed(TokenKind::IndexEq, start);
            } else {
                self.emit_fixed(TokenKind::Index, start);
            }
        } else {
            self.begin_group(TokenKind::OpenSquare, start);
        }
        Flow::Next
    }

    // --- group closing (official appendEndGroup/discardBeginGroupUntil) ---

    fn end_group(&mut self, close: TokenKind, start: usize) -> Flow {
        // Two-pass recovery (official tokenizeCloseBrace :1139-1153): in the
        // re-scan, a `}` whose stack head is the blamed `{` closes it with a
        // synthetic closer + report (unmatchedBeginGroup), and then goes on
        // to close the next begin down via the normal path below.
        if close == TokenKind::CloseCurly
            && let Some(offset) = self.curly_recovery
            && let Some(&begin_idx) = self.groups.last()
            && self.tokens[begin_idx as usize].kind == TokenKind::OpenCurly
            && self.tokens[begin_idx as usize].byte_offset == offset
        {
            self.groups.pop();
            self.report_unmatched_begin(begin_idx, start);
        }
        // `<` never matches an end token (official discardBeginGroupUntil).
        self.discard_open_lt();
        let opens = close.matching_opens();
        let Some(depth) = self
            .groups
            .iter()
            .rposition(|&idx| opens.contains(&self.tokens[idx as usize].kind))
        else {
            // No matching begin anywhere. `)`/`]`: leave the stack alone, the
            // token stands alone (official appendEndGroupInternal foundMatchingBrace
            // = false; oracle `{a);}`, `main() { a]; }` → no scanner diagnostic).
            // A `}` still closes off the stack head at the closer (oracle `f(}`
            // → expected_token ')' @ the `}`, nothing at eof); begins below the
            // head stay queued for eof handling.
            if close == TokenKind::CloseCurly
                && let Some(&begin_idx) = self.groups.last()
            {
                self.groups.pop();
                self.report_unmatched_begin(begin_idx, start);
            }
            self.emit(close, start, self.pos - start);
            return Flow::Next;
        };
        // Begins above the match: each is closed by a synthetic closer at the
        // mismatching closer and reported there (official insertSyntheticClosers;
        // oracle `var m = {[1};` → expected_token ']' @ the `}`).
        while self.groups.len() > depth + 1 {
            let begin_idx = self.groups.pop().unwrap();
            self.report_unmatched_begin(begin_idx, start);
        }
        let begin_idx = self.groups.pop().unwrap();
        let idx = self.emit(close, start, self.pos - start);
        self.end_of[begin_idx as usize] = idx;
        if close == TokenKind::CloseCurly
            && self.tokens[begin_idx as usize].kind == TokenKind::StringInterpolationExpression
        {
            return Flow::CloseInterp;
        }
        Flow::Next
    }

    /// Official `unmatchedBeginGroup`: the begin is closed by a synthetic
    /// closer at the mismatching closer's offset (inserted before the closer
    /// token, keeping `begin → endGroup` jumps well-formed) and reported as
    /// `expected_token` pinned at that closer, length 1 (analyzer
    /// translation `charOffset = begin.endToken.charOffset`; oracle
    /// `dart analyze --format=json` on `{[1};` → `expected_token` ']' @ the
    /// `}` offset, len 1).
    fn report_unmatched_begin(&mut self, begin_idx: u32, close_start: usize) {
        let begin_kind = self.tokens[begin_idx as usize].kind;
        let Some(close_kind) = begin_kind.matching_close() else {
            return;
        };
        self.emit_synthetic(close_kind, close_start, 0);
        let syn_idx = self.tokens.len() as u32 - 1;
        self.end_of[begin_idx as usize] = syn_idx;
        let lexeme = close_kind.fixed_lexeme().unwrap_or("}");
        self.diag(
            EXPECTED_TOKEN,
            || msg::expected_token(lexeme),
            close_start,
            close_start + 1,
        );
    }

    /// Closes every still-open group at EOF. Returns the token index of the
    /// single unclosed `{` when the stack was exactly one `{` (official
    /// `appendEofToken`'s `openBraceWithMissingEndForPossibleRecovery`
    /// condition, :532-537) — the two-pass recovery candidate.
    fn close_unclosed_groups(&mut self) -> Option<u32> {
        self.discard_open_lt();
        let curly_begin = match self.groups.as_slice() {
            [idx] if self.tokens[*idx as usize].kind == TokenKind::OpenCurly => Some(*idx),
            _ => None,
        };
        let eof = self.src.len();
        while let Some(begin_idx) = self.groups.pop() {
            let begin_kind = self.tokens[begin_idx as usize].kind;
            let Some(close_kind) = begin_kind.matching_close() else {
                continue;
            };
            let utf16 = self.utf16_value;
            let close_idx = self.tokens.len() as u32;
            self.tokens
                .push(Token::new(close_kind, eof as u32, 0, utf16).synthetic());
            self.end_of.push(crate::token::NO_END);
            self.end_of[begin_idx as usize] = close_idx;
            // analyzer translation: UnmatchedToken → expected_token, pinned AT
            // the eof position (oracle `dart analyze --format=json` on
            // `var x = {;` → expected_token '}' @11 = the eof offset; on
            // `f(😀` → @4 = the utf16 eof offset). A byte range cannot address
            // the unit past the source, so the encoded range is zero-width
            // at eof.
            let lexeme = close_kind.fixed_lexeme().unwrap_or("}");
            self.diag(EXPECTED_TOKEN, || msg::expected_token(lexeme), eof, eof);
        }
        curly_begin
    }
}
