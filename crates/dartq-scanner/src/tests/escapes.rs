//! String escape validation (official quote.dart `unescapeCodeUnits`): one
//! test per diagnostic code, well-formed silence pins, and the
//! multi-malformation decision-order pins.

use super::{assert_codes, assert_kinds, scan_src};
use crate::TokenKind as K;

fn one(src: &str) -> (Vec<&'static str>, u32, u32) {
    let r = scan_src(src);
    assert_eq!(r.diagnostics.len(), 1, "codes={:?}", super::codes(&r));
    let d = &r.diagnostics[0];
    (super::codes(&r), d.range.offset, d.range.length)
}

#[test]
fn wellformed_escapes_stay_silent() {
    // Full escape set incl. the boundary code point and uppercase hex digits.
    let full = r#"'\n\r\t\b\f\v\x41\xAB\u0041\u{1F600}\u{0}\u{10FFFF}\\\'\"\$'"#;
    for src in [full, r"'\u{ABCD} \x1F \$'", "'''a\\\nb'''"] {
        let r = scan_src(src);
        assert_kinds(&r, &[K::String]);
        assert!(r.diagnostics.is_empty(), "codes={:?}", super::codes(&r));
    }
    // Raw strings never validate escapes.
    for src in [r"r'\u{ZZ} \x \u'", r"r'''a\'''"] {
        let r = scan_src(src);
        assert!(r.diagnostics.is_empty(), "codes={:?}", super::codes(&r));
    }
}

#[test]
fn invalid_hex_escape() {
    // Corpus targeted/invalid_hex_escape.dart shape: one digit then the quote.
    let (codes, offset, length) = one(r"'\x0'");
    assert_eq!(codes, ["invalid_hex_escape"]);
    assert_eq!(offset, 1);
    assert_eq!(length, 3);
    // Analyzer error_parser_test parity: range covers `\x0`, not the bad char.
    let (codes, offset, length) = one(r"'not \x0 a'");
    assert_eq!(codes, ["invalid_hex_escape"]);
    assert_eq!(offset, 5);
    assert_eq!(length, 3);
    // No digits at all; bad first digit excludes it; bad second digit.
    assert_eq!(one(r"'\x'").1, 1);
    assert_eq!(one(r"'\xZ4'"), (vec!["invalid_hex_escape"], 1, 2));
    assert_eq!(one(r"'\x4Z'"), (vec!["invalid_hex_escape"], 1, 3));
    // The part must extend two UTF-16 units past `x` (official code-unit
    // counting): an astral character satisfies presence, its high unit is a
    // non-hex digit at d1 (range covers `\x` only), and after one hex digit
    // it is the non-hex digit at d2.
    assert_eq!(one(r"'\x😀'"), (vec!["invalid_hex_escape"], 1, 2));
    assert_eq!(one(r"'\x0😀'"), (vec!["invalid_hex_escape"], 1, 3));
    let r = scan_src(r"'\x0'");
    assert_eq!(
        r.diagnostics[0].message,
        "An escape sequence starting with '\\x' must be followed by 2 hexadecimal digits."
    );
}

#[test]
fn invalid_unicode_escape_u_started() {
    // Corpus targeted/invalid_unicode_escape_u_started.dart shape.
    assert_eq!(
        one(r"'ab\u'"),
        (vec!["invalid_unicode_escape_u_started"], 3, 2)
    );
    assert_eq!(
        one(r"'\u'"),
        (vec!["invalid_unicode_escape_u_started"], 1, 2)
    );
    // `$` starts an interpolation, so the part's content ends right after
    // `\u` there too.
    let r = scan_src(r"'\u$x'");
    assert_codes(&r, &["invalid_unicode_escape_u_started"]);
    assert_eq!(r.diagnostics[0].range.offset, 1);
    assert_eq!(r.diagnostics[0].range.length, 2);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationIdentifier,
            K::Identifier,
            K::String,
        ],
    );
    // Multiline closing triple quote acts as the part end.
    assert_eq!(
        one(r"'''\u'''"),
        (vec!["invalid_unicode_escape_u_started"], 3, 2)
    );
}

#[test]
fn invalid_unicode_escape_u_bracket() {
    // Corpus targeted/invalid_unicode_escape_u_bracket.dart shape: `\u{ZZ}`.
    // The official range includes the offending digit.
    assert_eq!(
        one(r"'\u{ZZ}'"),
        (vec!["invalid_unicode_escape_u_bracket"], 1, 4)
    );
    // Analyzer parity: content end after `{`, `{}`, mid-digit end, 7 digits
    // without `}` (range covers 6).
    assert_eq!(
        one(r"'\u{'"),
        (vec!["invalid_unicode_escape_u_bracket"], 1, 3)
    );
    assert_eq!(
        one(r"'\u{}'"),
        (vec!["invalid_unicode_escape_u_bracket"], 1, 4)
    );
    assert_eq!(
        one(r"'\u{0A'"),
        (vec!["invalid_unicode_escape_u_bracket"], 1, 5)
    );
    assert_eq!(
        one(r"'\u{0000000001}'"),
        (vec!["invalid_unicode_escape_u_bracket"], 1, 9)
    );
    let r = scan_src(r"'\u{ZZ}'");
    assert_eq!(
        r.diagnostics[0].message,
        "An escape sequence starting with '\\u{' must be followed by 1 to 6 hexadecimal digits followed by a '}'."
    );
}

#[test]
fn invalid_unicode_escape_u_no_bracket() {
    // Corpus targeted/invalid_unicode_escape_u_no_bracket.dart shape.
    assert_eq!(
        one(r"'\u12'"),
        (vec!["invalid_unicode_escape_u_no_bracket"], 1, 4)
    );
    // Analyzer parity: `\u04` (too few, range to the quote) and `\u0 and`
    // (bad digit excluded from the range).
    assert_eq!(
        one(r"'\u04'"),
        (vec!["invalid_unicode_escape_u_no_bracket"], 1, 4)
    );
    assert_eq!(
        one(r"'\u0 and some more'"),
        (vec!["invalid_unicode_escape_u_no_bracket"], 1, 3)
    );
    // Four units present, first not hex: range covers `\u` only.
    assert_eq!(
        one(r"'\uZZZZ'"),
        (vec!["invalid_unicode_escape_u_no_bracket"], 1, 2)
    );
}

#[test]
fn invalid_code_point() {
    // Corpus targeted/invalid_code_point.dart shape.
    let (codes, offset, length) = one(r"'\u{110000}'");
    assert_eq!(codes, ["invalid_code_point"]);
    assert_eq!(offset, 1);
    // The official range ends before the closing brace.
    assert_eq!(length, 9);
    let r = scan_src(r"'\u{110000}'");
    assert_eq!(
        r.diagnostics[0].message,
        "The escape sequence '\\u{...}' isn't a valid code point."
    );
    // Six hex digits above the cap; 0x10FFFF itself is valid.
    assert_eq!(one(r"'\u{200000}'"), (vec!["invalid_code_point"], 1, 9));
    let r = scan_src(r"'\u{10FFFF}'");
    assert!(r.diagnostics.is_empty());
}

#[test]
fn lone_backslash_cannot_stand_alone() {
    // EOF right after the backslash (corpus string2.dart:21 shape for the
    // multiline form): invalidEscapeStarted + unterminated, both on the
    // backslash.
    let r = scan_src(r"'ab\");
    assert_codes(
        &r,
        &[
            "invalid_unicode_escape_started",
            "unterminated_string_literal",
        ],
    );
    assert_eq!(r.diagnostics[0].range.offset, 3);
    assert_eq!(r.diagnostics[0].range.length, 1);
    assert_eq!(
        r.diagnostics[0].message,
        "The string '\\' can't stand alone."
    );
    // A trailing EOL leaves the backslash in the content the same way.
    let r = scan_src("'ab\\\n");
    assert_codes(
        &r,
        &[
            "invalid_unicode_escape_started",
            "unterminated_string_literal",
        ],
    );
    // Multiline at EOF (string2.dart:21: `æ` is multi-byte; the diagnostic
    // offset is UTF-16).
    let r = scan_src("return \"\"\"æbler\\");
    assert_codes(
        &r,
        &[
            "invalid_unicode_escape_started",
            "unterminated_string_literal",
        ],
    );
    assert_eq!(r.diagnostics[0].range.offset, 15);
    assert_eq!(r.diagnostics[0].range.length, 1);
}

#[test]
fn malformed_order_pins() {
    // Two malformations in one escape: the official missing-`}` report does
    // not return, so the code point check still fires — UBracket first, then
    // CodePoint, same range.
    let r = scan_src(r"'\u{1234567}'");
    assert_codes(
        &r,
        &["invalid_unicode_escape_u_bracket", "invalid_code_point"],
    );
    assert_eq!(r.diagnostics[0].range.offset, 1);
    assert_eq!(r.diagnostics[0].range.length, 9);
    assert_eq!(r.diagnostics[1].range.offset, 1);
    assert_eq!(r.diagnostics[1].range.length, 9);
    // Every other error returns: the first malformed escape wins and the
    // later one is not validated.
    let r = scan_src(r"'\xZZ \u{ZZ}'");
    assert_codes(&r, &["invalid_hex_escape"]);
    // ...except the missing-`}` fallthrough, which keeps validating: the
    // value stays under the cap, so the later `\xZZ` is reported too.
    let r = scan_src(r"'\u{0000000}\xZZ'");
    assert_codes(
        &r,
        &["invalid_unicode_escape_u_bracket", "invalid_hex_escape"],
    );
    // The code point error does return: the later `\xZZ` is not validated.
    let r = scan_src(r"'\u{110000}\xZZ'");
    assert_codes(&r, &["invalid_code_point"]);
}

#[test]
fn validation_resumes_per_part() {
    // Official unescaping runs per interpolation part; each part reports its
    // own first error.
    let r = scan_src(r"'\xZZ$x\xZZ'");
    assert_codes(&r, &["invalid_hex_escape", "invalid_hex_escape"]);
    assert_eq!(r.diagnostics[0].range.offset, 1);
    assert_eq!(r.diagnostics[1].range.offset, 7);
    let r = scan_src(r"'\xZZ${x}\xZZ'");
    assert_codes(&r, &["invalid_hex_escape", "invalid_hex_escape"]);
    assert_eq!(r.diagnostics[1].range.offset, 9);
}

#[test]
fn interpolation_truncates_digit_runs() {
    // `$` at a digit position ends the part: too-few-digits ranges reach it.
    assert_eq!(
        one(r"'\u04$x'"),
        (vec!["invalid_unicode_escape_u_no_bracket"], 1, 4)
    );
    assert_eq!(one(r"'\x4$x'"), (vec!["invalid_hex_escape"], 1, 3));
    // Content end inside the bracket digits.
    assert_eq!(
        one(r"'\u{$x}'"),
        (vec!["invalid_unicode_escape_u_bracket"], 1, 3)
    );
}

#[test]
fn multiline_quote_positions() {
    // A quote that does not close the string is ordinary content at a digit
    // position: `\u0'x` has five content units, the official presence check
    // fires with the range reaching the closing `'''`.
    assert_eq!(
        one(r"'''\u0'x'''"),
        (vec!["invalid_unicode_escape_u_no_bracket"], 3, 5)
    );
    assert_eq!(
        one(r"'''\u04'''"),
        (vec!["invalid_unicode_escape_u_no_bracket"], 3, 4)
    );
    assert_eq!(
        one(r"'''\u{4'''"),
        (vec!["invalid_unicode_escape_u_bracket"], 3, 4)
    );
    // Non-closing quote as a bracket digit: included in the official range.
    assert_eq!(
        one(r"'''\u{4'x'''"),
        (vec!["invalid_unicode_escape_u_bracket"], 3, 5)
    );
}
