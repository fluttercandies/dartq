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
//! (`upstream Dart SDK source, pkg/analyzer/lib/src/dart/scanner/translate_error_token.dart`):
//! unterminated strings/comments and missing digits pin to the **last UTF-16
//! unit** (`endOffset - 1`, `length: 1` — when the final character is astral
//! that is its low surrogate, which a byte range cannot address, so the
//! backoff is applied after conversion via [`ScanDiag::pin_last_unit`]),
//! illegal characters pin per UTF-16 surrogate (an astral character is two
//! single-unit diagnostics). Unmatched begin groups produce no scanner rows:
//! the official `UnmatchedToken` translation reads `begin.endGroup.charOffset`
//! POST-parse (a parser-side `moveSynthetic` relocation can move the closer),
//! so those rows are the parser's (the token table's unmatched pair list
//! carries the pairs).
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
/// by the finish pass). Cloned whole into recovery option runs (official
/// `createRecoveryOptionScanner`), whose diagnostics are discarded.
#[derive(Debug, Clone)]
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
///
/// Identical rows collapse to one: the oracle's row set is a
/// `Set<Diagnostic>` (`RecordingDiagnosticListener`; `Diagnostic ==`
/// compares code, offset, length, message) so distinct error tokens
/// translating to the same row surface once — the nested-interpolation EOF
/// cascade emits one unterminated-string error token per suspended body over
/// the same eof pin (oracle nested_unclosed 1:12 + 1:13: two rows over three
/// tokens). Identical rows share their byte span, so they are adjacent after
/// the sort and a byte-level `dedup_by` over `(code, span, pin, message)` is
/// the whole rule (byte equality ⇒ converted-row equality: the conversion is
/// a deterministic function of those fields).
///
/// Adjacency is structural, not incidental: identical rows can only come
/// from a single scanner event emitting several rows (the separator run
/// inside one number, the EOF unterminated cascade), and those events emit
/// nothing but identical rows between their iterations; every other site
/// emits at most one row per event, and `self.pos` is monotonic across the
/// single real pass (recovery-option runs discard their diagnostics, curly
/// recovery re-scans in a fresh lexer whose result replaces the first pass),
/// so no position is revisited by an event that could interleave a different
/// same-start row between identical ones — identical rows are always
/// consecutive pre-sort. Pinned observably by `tests::dedup` (oracle set
/// semantics over every multi-emission shape).
pub(crate) fn finish(mut diags: Vec<ScanDiag>, source: &str, file: FileId) -> Vec<Diagnostic> {
    diags.sort_by_key(|d| d.start);
    diags.dedup_by(|a, b| {
        a.code == b.code
            && a.start == b.start
            && a.end == b.end
            && a.pin_last_unit == b.pin_last_unit
            && a.message == b.message
    });
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
            // Saturating: the clamp above can land `end` at 0 (an over-long
            // target against an empty source), and a unit pin before the
            // text's first byte has no unit to back off onto — the range
            // degrades to `[0, 1)` rather than underflowing.
            (end.saturating_sub(1), 1)
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
///
/// A target more than one past the source clamps to the source's end (the
/// documented clamp-to-end semantics): recovery positions are the most
/// input-coupled slice arithmetic in the scanner, so an over-long target —
/// reachable only through a scanner bug — degrades the range to the end of
/// the text instead of slicing past it. Exactly one past the end is legal:
/// the official synthetic token at eof (missing_identifier after a `$` at
/// eof, upstream Dart SDK source,
/// `pkg/analyzer/lib/src/dart/scanner/translate_error_token.dart`) reports
/// `length: 1` at the eof boundary — one virtual UTF-16 unit past the text.
fn walk(source: &str, cursor: &mut (usize, u32), target: u32) -> u32 {
    let len = source.len() as u32;
    let virtual_end = target > len && target - len == 1;
    let target = target.min(len);
    while (cursor.0 as u32) < target {
        // The clamp keeps every target within the source and the cursor is
        // always on a char boundary here, so a char exists to read; a dry
        // cursor breaks to the clamp-to-end answer instead of unwrapping.
        let Some(ch) = source[cursor.0..].chars().next() else {
            break;
        };
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
    if virtual_end { cursor.1 + 1 } else { cursor.1 }
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

/// Scanner diagnostic message texts. The oracle's MESSAGE field is the
/// analyzer-side text each error token translates to
/// (`translate_error_token.dart` → analyzer diagnostic registry, locked
/// verbatim in `dartq-diagnostics` `data/syntax-codes.tsv`); these texts
/// mirror it, pinned by the scanner's message probe tests.
pub(crate) mod msg {
    /// The registry's single illegal-character template: `Illegal character
    /// '{0}'.` with `{0}` the decimal UTF-16 code unit the scanner sees
    /// (analyzer `messages.yaml` `illegalCharacter`, parameter
    /// `int codePoint`; locked in `dartq-diagnostics` `data/syntax-codes.tsv`
    /// as `Illegal character '{0}'.`). One wording for every token class —
    /// control characters, non-ASCII whitespace, non-ASCII identifier
    /// characters, and each surrogate of an astral character (the UTF-16
    /// scanners see each code unit separately; oracle captures:
    /// `Illegal character '1075'.`, `'55357'.` / `'56832'.`).
    pub fn illegal_character(cp: u32) -> String {
        format!("Illegal character '{cp}'.")
    }

    pub const UNTERMINATED_STRING: &str = "Unterminated string literal.";
    pub const UNTERMINATED_COMMENT: &str = "Unterminated multi-line comment.";
    pub const MISSING_DIGIT: &str = "Decimal digit expected.";
    pub const MISSING_HEX_DIGIT: &str = "Hexadecimal digit expected.";
    pub const UNEXPECTED_SEPARATOR_IN_NUMBER: &str =
        "Digit separators ('_') in a number literal can only be placed between two digits.";
    pub const MISSING_IDENTIFIER: &str = "Expected an identifier.";

    /// String escape validations (quote.dart unescape, verbatim from the
    /// registry). `INVALID_CODE_POINT`'s registry template fills its `{0}`
    /// with the literal `\u{...}` (analyzer `error_converter.dart`).
    pub const INVALID_ESCAPE_STARTED: &str = "The string '\\' can't stand alone.";
    pub const INVALID_HEX_ESCAPE: &str =
        "An escape sequence starting with '\\x' must be followed by 2 hexadecimal digits.";
    pub const INVALID_UNICODE_ESCAPE_U_STARTED: &str = "An escape sequence starting with '\\u' must be followed by 4 hexadecimal digits or from 1 to 6 digits between '{' and '}'.";
    pub const INVALID_UNICODE_ESCAPE_U_BRACKET: &str = "An escape sequence starting with '\\u{' must be followed by 1 to 6 hexadecimal digits followed by a '}'.";
    pub const INVALID_UNICODE_ESCAPE_U_NO_BRACKET: &str =
        "An escape sequence starting with '\\u' must be followed by 4 hexadecimal digits.";
    pub const INVALID_CODE_POINT: &str = "The escape sequence '\\u{...}' isn't a valid code point.";

    /// The `unsupported_operator` registry template with `{0}` filled by the
    /// unsupported lexeme (both `===` and `!==` route here).
    pub fn unsupported_operator(lexeme: &str) -> String {
        format!("The '{lexeme}' operator is not supported.")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dartq_source::{FileTable, SourceRange};

    use super::{ScanDiag, UNTERMINATED_MULTI_LINE_COMMENT, finish, walk};

    fn file() -> dartq_source::FileId {
        FileTable::new().intern(Path::new("t.dart"))
    }

    /// The walk clamp (stability lane): recovery positions are the most
    /// input-coupled slice arithmetic in the scanner, so an over-long
    /// target — reachable only through a scanner bug — clamps to the
    /// source's end (the documented clamp-to-end semantics) instead of
    /// slicing past it. Healthy targets are unaffected: every in-bounds
    /// position keeps its exact UTF-16 offset, and exactly one past the end
    /// is the legal synthetic-at-eof virtual unit.
    #[test]
    fn over_long_walk_target_clamps_to_the_source_end() {
        assert_eq!(walk("ab", &mut (0, 0), 1), 1, "in-bounds target exact");
        assert_eq!(
            walk("ab", &mut (0, 0), 3),
            3,
            "exactly one past the end: the synthetic-at-eof virtual unit"
        );
        assert_eq!(
            walk("ab", &mut (0, 0), u32::MAX),
            2,
            "over-long target clamps to the UTF-16 end"
        );
        assert_eq!(
            walk("", &mut (0, 0), u32::MAX),
            0,
            "empty source clamps to zero"
        );
    }

    /// The same clamp through the finish pass, both range shapes: a plain
    /// range degrades to the clamped end, and a unit pin before the text's
    /// first byte saturates instead of underflowing.
    #[test]
    fn over_long_finish_targets_degrade_without_panicking() {
        let pin = |pin_last_unit| {
            finish(
                vec![ScanDiag {
                    code: UNTERMINATED_MULTI_LINE_COMMENT,
                    message: String::new(),
                    start: 0,
                    end: u32::MAX,
                    pin_last_unit,
                }],
                "ab",
                file(),
            )
            .remove(0)
            .range
        };
        assert_eq!(
            pin(false),
            SourceRange {
                offset: 0,
                length: 2
            }
        );
        assert_eq!(
            pin(true),
            SourceRange {
                offset: 1,
                length: 1
            }
        );
    }
}
