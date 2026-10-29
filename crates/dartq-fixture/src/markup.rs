//! The inline markup scanner: a byte-exact port of the upstream linter's
//! `TestCode` grammar (`pkg/analyzer/lib/src/test_utilities/test_code_
//! format.dart`, upstream Dart SDK source), the format every
//! `assertDiagnosticsFromMarkup` body is written in.
//!
//! Grammar (matched in the upstream order — the order decides the `/**/`
//! escapes):
//!
//! ```text
//! '^'                    position 0 (stripped; carries no expectation)
//! '/*' digits '*/'       numbered position (stripped)
//! '[!' .. '!]'           shorthand range 0
//! '/*[' digits '*/'      numbered range start
//! '/*' digits ']*/'      numbered range end
//! '/**/'                 zero-width escape: removed; '^/**/' emits a
//!                       literal '^', '[/**/!' a literal '[!', '!/**/]' a
//!                       literal '!]'
//! ```
//!
//! Numbered positions and range starts must be consecutive from 0; a range
//! end must follow its start; every start must have its end — the same
//! validity rules the upstream parser enforces with `ArgumentError`, kept
//! here as [`MarkupError`] so a malformed fixture fails with a reason
//! instead of a wrong-but-green test.
//!
//! Ranges are reported over the **stripped** code (markers removed), in
//! UTF-16 code units — the coordinate domain of the analyzer's
//! `SourceRange` and of dartq's `Lint::range`. (Dart's `String.length` is
//! a UTF-16 code unit count, so the upstream `codeBuffer.length` offsets
//! are UTF-16 too; this scanner counts them as it emits.)

use core::ops::Range;

/// One stripped range: the UTF-16 span in the unmarked code plus the marked
/// text (the upstream `TestCodeRange.text`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkedRange {
    pub range: Range<u32>,
    pub text: String,
}

/// Why a marked source could not be stripped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkupError {
    /// A duplicated or non-consecutive number, a startless end, or an
    /// endless start (which of the four, in prose).
    Invalid(String),
}

/// One markup token: what a matched marker means, minus the emitted
/// character (only the `^/**/` escape emits one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// `^/**/` — a literal `^` in the stripped code.
    CaretEscape,
    /// `^` or `/*N*/` — a position, stripped, carrying no expectation.
    Position(usize),
    /// `[!` or `/*[N*/`.
    RangeStart(usize),
    /// `!]` or `/*N]*/`.
    RangeEnd(usize),
    /// `/**/` alone — removed.
    ZeroWidth,
}

/// The longest markup token at the head of `rest`, with its byte length —
/// a pure lex (no validation), mirroring the upstream scan order.
fn next_token(rest: &str) -> Option<(usize, Token)> {
    if rest.starts_with("^/**/") {
        return Some((5, Token::CaretEscape));
    }
    if rest.starts_with('^') {
        return Some((1, Token::Position(0)));
    }
    if let Some((digits, len)) = numbered(rest, "/*", "*/") {
        return Some((len, Token::Position(digits)));
    }
    if rest.starts_with("[!") {
        return Some((2, Token::RangeStart(0)));
    }
    if rest.starts_with("!]") {
        return Some((2, Token::RangeEnd(0)));
    }
    if let Some((digits, len)) = numbered(rest, "/*[", "*/") {
        return Some((len, Token::RangeStart(digits)));
    }
    if let Some((digits, len)) = numbered(rest, "/*", "]*/") {
        return Some((len, Token::RangeEnd(digits)));
    }
    if rest.starts_with("/**/") {
        return Some((4, Token::ZeroWidth));
    }
    None
}

/// Matches `prefix digits suffix` at the head of `rest`: the digits value
/// and the whole match's byte length. An unparseable number (overflow) is
/// not a marker.
fn numbered(rest: &str, prefix: &str, suffix: &str) -> Option<(usize, usize)> {
    let after = rest.strip_prefix(prefix)?;
    let end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    if end == 0 {
        return None;
    }
    let digits = after[..end].parse::<usize>().ok()?;
    after[end..]
        .strip_prefix(suffix)
        .map(|_| (digits, prefix.len() + end + suffix.len()))
}

/// The upstream `positionOffsets` / `rangeStartOffsets` /
/// `rangeEndOffsets` maps (number → recorded offset), one `Vec<Option>`
/// each: index = number, `Some(offset)` = recorded.
#[derive(Default)]
struct Numbers {
    slots: Vec<Option<u32>>,
}

impl Numbers {
    /// The upstream `recordPosition` / `recordRangeStart` prelude: a number
    /// arrives consecutively from 0 and never twice; on success the slot
    /// exists (still empty until `set`).
    fn admit(&mut self, number: usize) -> Result<(), MarkupError> {
        match self.slots.get(number) {
            Some(Some(_)) => Err(MarkupError::Invalid(format!(
                "number {number} recorded twice"
            ))),
            Some(None) => Ok(()),
            None if number > self.slots.len() => Err(MarkupError::Invalid(format!(
                "number {number} but expected {}",
                self.slots.len()
            ))),
            None => {
                self.slots.push(None);
                Ok(())
            }
        }
    }
    fn recorded(&self, number: usize) -> bool {
        self.slots.get(number).is_some_and(Option::is_some)
    }
    fn set(&mut self, number: usize, offset: u32) {
        self.slots[number] = Some(offset);
    }
}

/// Strips markup from `marked`, returning the unmarked code and its ranges
/// (in number order — the upstream map iteration order; callers compare as
/// sets).
///
/// # Errors
///
/// The upstream `ArgumentError` conditions, as [`MarkupError::Invalid`].
pub fn strip(marked: &str) -> Result<(String, Vec<MarkedRange>), MarkupError> {
    let mut code = String::with_capacity(marked.len());
    let mut utf16 = 0u32;
    let mut positions = Numbers::default();
    let mut starts = Numbers::default();
    let mut ends = Numbers::default();
    let mut pos = 0usize;
    while pos < marked.len() {
        let rest = &marked[pos..];
        let Some((len, token)) = next_token(rest) else {
            let ch = rest.chars().next().expect("non-empty remainder");
            code.push(ch);
            utf16 += ch.len_utf16() as u32;
            pos += ch.len_utf8();
            continue;
        };
        match token {
            Token::CaretEscape => code.push('^'),
            Token::Position(n) => {
                positions.admit(n)?;
                // Recorded like the upstream map (the value is the offset;
                // positions carry no expectation, but the duplicate rule
                // must hold).
                positions.set(n, utf16);
            }
            Token::RangeStart(n) => {
                starts.admit(n)?;
                starts.set(n, utf16);
            }
            Token::RangeEnd(n) => {
                if ends.recorded(n) {
                    return Err(MarkupError::Invalid(format!(
                        "range end numbered {n} recorded twice"
                    )));
                }
                if !starts.recorded(n) {
                    return Err(MarkupError::Invalid(format!(
                        "range end numbered {n} without a preceding start"
                    )));
                }
                while ends.slots.len() <= n {
                    ends.slots.push(None);
                }
                ends.set(n, utf16);
            }
            Token::ZeroWidth => {}
        }
        pos += len;
    }
    let mut ranges = Vec::new();
    for (number, slot) in starts.slots.iter().enumerate() {
        let Some(start) = slot else { continue };
        let Some(Some(end)) = ends.slots.get(number) else {
            return Err(MarkupError::Invalid(format!(
                "range start numbered {number} without an end"
            )));
        };
        ranges.push(MarkedRange {
            range: *start..*end,
            text: slice_utf16(&code, *start, *end),
        });
    }
    Ok((code, ranges))
}

/// The marked text between two UTF-16 offsets of the stripped code (the
/// upstream `TestCodeRange.text`); marker spans never split a character.
fn slice_utf16(code: &str, start: u32, end: u32) -> String {
    let mut byte_start = code.len();
    let mut byte_end = code.len();
    let mut byte = 0usize;
    let mut u16_at = 0u32;
    for ch in code.chars() {
        if u16_at == start {
            byte_start = byte;
        }
        if u16_at == end {
            byte_end = byte;
        }
        byte += ch.len_utf8();
        u16_at += ch.len_utf16() as u32;
    }
    // A range ending at the text's end never hits `u16_at == end` inside
    // the loop (no character starts there).
    if u16_at == end {
        byte_end = byte;
    }
    code[byte_start..byte_end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The doc example: shorthand range, stripped code, UTF-16 offsets.
    #[test]
    fn shorthand_range_strips_to_utf16_offsets() {
        let (code, ranges) = strip("else [!;!]\n").expect("strips");
        assert_eq!(code, "else ;\n");
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].range, 5..6);
        assert_eq!(ranges[0].text, ";");
    }

    /// `[!!p.isEmpty!]` — the shorthand wrapping content that itself
    /// starts with `!` (the prefer_is_not_empty shape).
    #[test]
    fn shorthand_range_with_leading_bang_content() {
        let (_, ranges) = strip("var x = [!!p.isEmpty!];\n").expect("strips");
        assert_eq!(ranges[0].range, 8..18);
        assert_eq!(ranges[0].text, "!p.isEmpty");
    }

    /// Numbered ranges pair by number, consecutive from 0.
    #[test]
    fn numbered_ranges_pair_by_number() {
        let marked = "case A(a: int /*[0*/a_b/*0]*/):\ncase A(a: int /*[1*/a_b/*1]*/?):\n";
        let (code, ranges) = strip(marked).expect("strips");
        assert_eq!(code, "case A(a: int a_b):\ncase A(a: int a_b?):\n");
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            ranges[0].range,
            14..17,
            "`a_b` starts after `case A(a: int `"
        );
        assert_eq!(ranges[0].text, "a_b");
        assert_eq!(ranges[1].text, "a_b");
        let second_line = "case A(a: int a_b):\n".len() as u32;
        assert_eq!(ranges[1].range, second_line + 14..second_line + 17);
    }

    /// The `/**/` escape family: literal `^`, literal `[!`, literal `!]`,
    /// and the bare marker removal.
    #[test]
    fn zero_width_marker_escapes_literals() {
        let (code, ranges) = strip("a[/**/! b !/**/] c /**/ d^/**/e").expect("strips");
        assert_eq!(code, "a[! b !] c  d^e");
        assert!(ranges.is_empty(), "{ranges:?}");
    }

    /// Positions strip without any expectation record. A `^` is number 0
    /// — a `/*0*/` after it is a duplicate, exactly as upstream.
    #[test]
    fn positions_strip_silently() {
        let (code, ranges) = strip("int ^x = /*1*/y;").expect("strips");
        assert_eq!(code, "int x = y;");
        assert!(ranges.is_empty());
    }

    /// The upstream validity rules, each failing with a reason.
    #[test]
    fn invalid_markups_fail_with_reasons() {
        assert!(strip("[!a!][!b!]").is_err(), "duplicate shorthand start");
        assert!(strip("/*[1*/a/*1]*/").is_err(), "numbers must start at 0");
        assert!(strip("a/*0]*/").is_err(), "end without start");
        assert!(strip("/*[0*/a").is_err(), "start without end");
        assert!(strip("/*0*/x/*0*/").is_err(), "duplicate position 0");
    }

    /// Offsets count UTF-16 code units, not bytes: an emoji before the
    /// marker costs two.
    #[test]
    fn offsets_are_utf16_code_units() {
        let (code, ranges) = strip("var s = '😀'; [!x!];").expect("strips");
        assert_eq!(code, "var s = '😀'; x;");
        // The emoji occupies UTF-16 9..11, so the `x` sits at 14.
        assert_eq!(ranges[0].range, 14..15);
    }
}
