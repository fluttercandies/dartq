//! Scanner diagnostics: byte-offset bookkeeping during scanning, converted
//! to authoritative [`Diagnostic`]s in a finish pass.
//!
//! Scanning records diagnostics as byte offsets (conversion is deferred to
//! the finish pass so the cursor may move backwards); the finish pass sorts
//! by byte start and converts to UTF-16 [`SourceRange`]s with one monotonic
//! cursor for the starts (each end is converted from its start point, so
//! overlapping same-start ranges stay intact), constructing
//! `dartq_diagnostics::Diagnostic`s (severity is always
//! [`Severity::Error`], type always `DiagnosticType::SyntacticError`,
//! matching the official scanner's errors).
//!
//! Diagnostic range pinning mirrors the analyzer's translation of
//! `ErrorToken`s
//! (`.third/dart-sdk/pkg/analyzer/lib/src/dart/scanner/translate_error_token.dart`):
//! unterminated strings/comments and missing digits pin to the **last UTF-16
//! unit** (`endOffset - 1`, `length: 1` — when the final character is astral
//! that is its low surrogate, which a byte range cannot address, so the
//! backoff is applied after conversion via [`ScanDiag::pin_last_unit`]),
//! illegal characters pin per UTF-16 surrogate (an astral character is two
//! single-unit diagnostics), group mismatches pin to the mismatching closer
//! token (`begin.endToken.charOffset`), and unclosed groups at EOF pin to
//! the eof position itself (oracle `dart analyze --format=json`: offset =
//! source length; encoded zero-width since no byte range reaches past the
//! source).
//!
//! Truncating casts are allowed: source length ≤ `u32::MAX` is the
//! dartq-source engine contract.

#![allow(clippy::cast_possible_truncation)]

use dartq_diagnostics::{CodeInfo, Diagnostic, Severity};
use dartq_source::{FileId, SourceRange};

/// Scanner error codes of the official `ScannerErrorCode` family (verified
/// against `_fe_analyzer_shared-91.0.0` `scanner/errors.g.dart`). The
/// build-generated table in `dartq-diagnostics` (data/syntax-codes.tsv) is
/// the single source of truth; these are re-exported so the scanner's public
/// API and internal `crate::diag::` paths stay stable.
pub use dartq_diagnostics::{
    ILLEGAL_CHARACTER, MISSING_DIGIT, MISSING_HEX_DIGIT, UNEXPECTED_SEPARATOR_IN_NUMBER,
    UNSUPPORTED_OPERATOR, UNTERMINATED_MULTI_LINE_COMMENT,
};

/// A diagnostic recorded during scanning (byte offsets; converted to UTF-16
/// by the finish pass).
#[derive(Debug)]
pub(crate) struct ScanDiag {
    pub code: CodeInfo,
    pub message: String,
    pub start: u32,
    pub end: u32,
    /// Pins the range to the single UTF-16 unit ending at the converted
    /// `end` (`[end-1, end)`), ignoring `start`: the official `endOffset - 1,
    /// length 1` translation. When the final character is astral (2 UTF-16
    /// units) that unit is its low surrogate, which no byte range can
    /// address, so the backoff happens after conversion.
    pub pin_last_unit: bool,
}

/// Converts the recorded diagnostics in a single pass and emits the
/// authoritative list (stable sort by byte start; the relative order of
/// same-start diagnostics follows emission order, matching the official
/// multi-diagnostic output).
pub(crate) fn finish(mut diags: Vec<ScanDiag>, source: &str, file: FileId) -> Vec<Diagnostic> {
    diags.sort_by_key(|d| d.start);
    let mut out = Vec::with_capacity(diags.len());
    let mut cursor = (0usize, 0u32); // (bytes converted so far, their UTF-16 offset)
    for d in diags {
        // The end is converted from the **pre-start** cursor state: start and
        // end conversions are independent, so a start landing inside a
        // character (a `length: 1` unit pin) cannot desync the end value of
        // the same diagnostic. Walking the end through the already-advanced
        // cursor would instead overshoot for pin-style ranges (the second+
        // same-start range would collapse to zero length — the F2 regression).
        let mut end_cursor = cursor;
        let offset = walk(source, &mut cursor, d.start);
        let end = walk(source, &mut end_cursor, d.end);
        let (offset, length) = if d.pin_last_unit {
            (end - 1, 1)
        } else {
            (offset, end - offset)
        };
        out.push(Diagnostic::new(
            Severity::Error,
            d.code,
            d.message,
            file,
            SourceRange { offset, length },
            None,
        ));
    }
    out
}

/// Advances the cursor to the target byte offset (sorting guarantees
/// monotonicity) and returns its UTF-16 offset. A target landing inside a
/// character counts that character as one UTF-16 unit instead of its full
/// width: byte ranges like `at..at+1` mean "one unit at `at`" (the official
/// `length: 1` pins), which for a partially covered astral character is one
/// unit, not its surrogate-pair width. The shared cursor itself keeps full
/// unit accounting across such a landing, so a later diagnostic targeting
/// the same character's end (the low surrogate of an astral illegal
/// character) stays in sync.
fn walk(source: &str, cursor: &mut (usize, u32), target: u32) -> u32 {
    while (cursor.0 as u32) < target {
        let ch = source[cursor.0..].chars().next().unwrap();
        let char_end = cursor.0 + ch.len_utf8();
        if (char_end as u32) <= target {
            cursor.0 = char_end;
            cursor.1 += ch.len_utf16() as u32;
        } else {
            let at = cursor.1 + 1;
            cursor.0 = char_end;
            cursor.1 += ch.len_utf16() as u32;
            return at;
        }
    }
    cursor.1
}

/// The non-ASCII whitespace set (the official `error_token.dart` General
/// Punctuation list; matched characters are skipped rather than folded into
/// identifiers).
pub(crate) fn is_unicode_space(ch: char) -> bool {
    matches!(
        ch,
        '\u{00A0}' | '\u{1680}' | '\u{180E}' | '\u{2000}'
            ..='\u{200B}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// Official `messages.yaml` template texts (differential testing compares
/// only `(code, file)`; message wording stays close to the official text).
pub(crate) mod msg {
    pub fn illegal_character(ch: char) -> String {
        let cp = ch as u32;
        // The wording boundary follows the official token-class boundary
        // (error_token.dart:17 `character < 0x1f` → AsciiControlCharacterToken;
        // 0x1F/0x7F are NonAsciiIdentifierToken, i.e. "non-ASCII character" wording).
        if cp < 0x1f {
            format!("The control character U+{cp:04X} can only be used in strings and comments.")
        } else if super::is_unicode_space(ch) {
            format!("The non-ASCII space character {cp} can only be used in strings and comments.")
        } else {
            format!(
                "The non-ASCII character '{ch}' ({cp}) can't be used in identifiers, only in strings and comments."
            )
        }
    }

    /// Message for one UTF-16 surrogate of an astral illegal character (the
    /// official scanners see each code unit separately, so each half is
    /// reported with its own decimal code point — oracle: `Illegal character
    /// '55357'.` / `'56832'.`; a lone surrogate has no glyph to quote).
    pub fn illegal_character_cp(cp: u32) -> String {
        format!(
            "The non-ASCII character '{cp}' can't be used in identifiers, only in strings and comments."
        )
    }

    pub fn expected_token(lexeme: &str) -> String {
        format!("Expected to find '{lexeme}'.")
    }

    pub fn unterminated_string(open: &str, close: &str) -> String {
        format!("String starting with {open} must end with {close}.")
    }

    pub const UNTERMINATED_COMMENT: &str = "Unterminated multi-line comment.";
    pub const MISSING_HEX_DIGIT: &str = "A hex digit (0-9, a-f, or A-F) is expected after '0x'.";
    pub const MISSING_EXPONENT: &str =
        "Numbers in exponential notation have to have digits after the 'e'.";
    pub const SEPARATOR_IN_NUMBER: &str =
        "Separator '_' is not allowed in the middle, the beginning, or the end of a number.";
    pub const UNSUPPORTED_OPERATOR: &str = "The '===' is not a valid operator.";
    pub const MISSING_IDENTIFIER: &str = "Expected an identifier.";
    pub const UNSUPPORTED_NEQ: &str = "The '!==' is not a valid operator.";
}
