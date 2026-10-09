//! Number form matrix and the full string form matrix (raw/triple-quoted/nested interpolation/escapes/unterminated).

use super::{assert_codes, assert_kinds, scan_src};
use crate::{Keyword, TokenKind as K};
use dartq_source::byte_offset_to_utf16;

#[test]
fn int_forms() {
    assert_kinds(&scan_src("0 1 123 0009"), &[K::Int; 4]);
    // Valid digit separators.
    assert_kinds(&scan_src("1_000 0x1_FF"), &[K::Int, K::Hexadecimal]);
    let r = scan_src("1_000");
    assert_eq!(r.tokens.first().byte_len, 5);
}

#[test]
fn int_separator_errors() {
    // Trailing separator.
    let r = scan_src("1_");
    assert_kinds(&r, &[K::Int]);
    assert_codes(&r, &["unexpected_separator_in_number"]);
    // Consecutive separators `1__2`: a following digit resets prev_sep; the official
    // scanner does not report (tokenizeNumber only checks previousWasSeparator at
    // the e/./end checkpoints).
    let r = scan_src("1__2");
    assert_kinds(&r, &[K::Int]);
    assert!(r.diagnostics.is_empty());
    // `1_000_` also reports as a trailing separator.
    let r = scan_src("1_000_");
    assert_codes(&r, &["unexpected_separator_in_number"]);
}

#[test]
fn hex_forms() {
    assert_kinds(&scan_src("0xFF 0x0 0XaB"), &[K::Hexadecimal; 3]);
    // `0x` with no digits: synthetic `0` + missing_hex_digit.
    let r = scan_src("0x");
    assert_kinds(&r, &[K::Hexadecimal]);
    assert!(r.tokens.first().is_synthetic());
    assert_codes(&r, &["missing_hex_digit"]);
    // Diagnostic pinned to the last character `x` (offset 1).
    assert_eq!(r.diagnostics[0].range.offset, 1);
    assert_eq!(r.diagnostics[0].range.length, 1);
    // Identifier after `0x`: recovery continues.
    let r = scan_src("0xg");
    assert_kinds(&r, &[K::Hexadecimal, K::Identifier]);
    assert_codes(&r, &["missing_hex_digit"]);
}

#[test]
fn double_forms() {
    assert_kinds(
        &scan_src("1.5 .5 1e3 1E+3 1e-3 1.5e10 0.5"),
        &[K::Double; 7],
    );
}

#[test]
fn missing_exponent() {
    // `1e`: synthetic DOUBLE + missing_digit (pinned to `e`).
    let r = scan_src("1e");
    assert_kinds(&r, &[K::Double]);
    assert!(r.tokens.first().is_synthetic());
    assert_codes(&r, &["missing_digit"]);
    assert_eq!(r.diagnostics[0].range.offset, 1);
    // `1e+` likewise.
    let r = scan_src("1e+");
    assert_codes(&r, &["missing_digit"]);
}

#[test]
fn one_dot_is_int_plus_period() {
    // `1.` = INT + PERIOD (official tokenizeNumber, non-digit-after-number branch).
    assert_kinds(&scan_src("1. x"), &[K::Int, K::Period, K::Identifier]);
    // `1x` = INT + IDENTIFIER, no diagnostic (official behavior).
    let r = scan_src("1x");
    assert_kinds(&r, &[K::Int, K::Identifier]);
    assert!(r.diagnostics.is_empty());
    // `0x1g` = HEX + IDENTIFIER.
    assert_kinds(&scan_src("0x1g"), &[K::Hexadecimal, K::Identifier]);
}

#[test]
fn string_basic_forms() {
    assert_kinds(&scan_src("'' \"\" 'a' \"a b\""), &[K::String; 4]);
    let r = scan_src("'hi'");
    assert_eq!(r.tokens.first().byte_range(), 0..4);
    assert_eq!(r.tokens.first().byte_offset, 0);
    assert_eq!(r.tokens.first().utf16_offset, 0);
}

#[test]
fn raw_strings() {
    assert_kinds(
        &scan_src("r'a' r\"b\" r'''c''' r\"\"\"d\"\"\""),
        &[K::String; 4],
    );
    // Uppercase R does not trigger raw: identifier + string.
    assert_kinds(&scan_src("R'x'"), &[K::Identifier, K::String]);
    // Raw does not escape: \n stays literal.
    let r = scan_src("r'a\\nb'");
    assert_kinds(&r, &[K::String]);
    assert_eq!(r.tokens.first().byte_len, 7); // all 7 bytes of r'a\nb'
    assert!(r.diagnostics.is_empty());
}

#[test]
fn multiline_strings() {
    let src = "'''a\nb'''";
    let r = scan_src(src);
    assert_kinds(&r, &[K::String]);
    assert_eq!(r.tokens.first().byte_len as usize, src.len());
    // Newlines are legal inside multiline strings.
    let src = "\"\"\"line1\nline2\"\"\"";
    assert_kinds(&scan_src(src), &[K::String]);
    // An escaped quote does not terminate a multiline string.
    assert_kinds(&scan_src("'''a\\'b'''"), &[K::String]);
}

#[test]
fn escape_sequences() {
    // Escape set: \n \r \t \b \f \v \xHH \uXXXX \u{...} \\ \' \" \$.
    let src = r#"'\n\r\t\b\f\v\x41\u0041\u{1F600}\\\'\"\$'"#;
    let r = scan_src(src);
    assert_kinds(&r, &[K::String]);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn unterminated_single_line() {
    let r = scan_src("'abc");
    // Synthetic empty STRING segment + unterminated_string_literal (pinned to the last character `c`).
    assert_eq!(
        r.tokens
            .as_slice()
            .iter()
            .filter(|t| t.kind == K::String)
            .count(),
        1
    );
    assert!(r.tokens.first().is_synthetic());
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 3);
    assert_eq!(r.diagnostics[0].range.length, 1);
    // EOL terminates scanning; the second line's `c` and the lone `'` are separate tokens — the latter is also unterminated.
    let r = scan_src("'ab\nc'");
    assert_codes(
        &r,
        &["unterminated_string_literal", "unterminated_string_literal"],
    );
    assert_kinds(&r, &[K::String, K::Identifier, K::String]);
}

#[test]
fn unterminated_raw() {
    let r = scan_src("r'abc");
    assert_codes(&r, &["unterminated_string_literal"]);
    let r = scan_src("r'ab\nc");
    assert_codes(&r, &["unterminated_string_literal"]);
}

#[test]
fn unterminated_multiline() {
    let r = scan_src("'''abc\ndef");
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.length, 1);
}

#[test]
fn interpolation_expression_form() {
    // 'a${b}c' → String("'a$") ${ Ident(b) } String("c'")
    let src = "'a${b}c'";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::CloseCurly,
            K::String,
        ],
    );
    let t = &r.tokens;
    // The prefix segment ends BEFORE the `$` (official corpus
    // scanner_test.dart:1034-1051: `"Hello "` [0..7), `${` at 7).
    assert_eq!(t.lexeme(src, 0), "'a");
    assert_eq!(t.lexeme(src, 1), "${");
    assert_eq!(t.lexeme(src, 3), "}");
    assert_eq!(t.lexeme(src, 4), "c'");
    // The `${` begin-group pairs with `}`.
    assert_eq!(t.matching_end(1), Some(3));
}

#[test]
fn interpolation_identifier_form() {
    // 'a$b.c' → String("'a$") $(SII) Ident(b) String(".c'")
    let src = "'a$b.c'";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationIdentifier,
            K::Identifier,
            K::String,
        ],
    );
    let t = &r.tokens;
    assert_eq!(t.lexeme(src, 0), "'a");
    assert_eq!(t.lexeme(src, 3), ".c'");
}

#[test]
fn interpolation_keyword_identifier() {
    // '$this' allows keywords (official tokenizeKeywordOrIdentifier).
    let src = "'$this x'";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationIdentifier,
            K::Keyword(Keyword::This),
            K::String,
        ],
    );
}

#[test]
fn interpolation_dollar_alone() {
    // '$' → synthetic empty Identifier + missing_identifier.
    let r = scan_src("'$'");
    assert_codes(&r, &["missing_identifier"]);
    let idents: Vec<_> = r
        .tokens
        .as_slice()
        .iter()
        .filter(|t| t.kind == K::Identifier)
        .collect();
    assert_eq!(idents.len(), 1);
    assert!(idents[0].is_synthetic());
    assert_eq!(idents[0].byte_len, 0);
}

#[test]
fn interpolation_nested_expression() {
    // Nested interpolation: ${'a${b}c'} — a linear flat stream.
    let src = "'x${'a${b}c'}y'";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::String,                        // 'x$
            K::StringInterpolationExpression, // ${
            K::String,                        // 'a$
            K::StringInterpolationExpression, // ${
            K::Identifier,                    // b
            K::CloseCurly,                    // }
            K::String,                        // c'
            K::CloseCurly,                    // }
            K::String,                        // y'
        ],
    );
    assert!(r.diagnostics.is_empty());
    let t = &r.tokens;
    assert_eq!(t.matching_end(1), Some(7)); // outer ${ → the final }
    assert_eq!(t.matching_end(3), Some(5)); // inner ${ → inner }
}

#[test]
fn interpolation_with_function_call() {
    let src = "'v=${f(1)}!'";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::OpenParen,
            K::Int,
            K::CloseParen,
            K::CloseCurly,
            K::String,
        ],
    );
    assert!(r.diagnostics.is_empty());
}

#[test]
fn multiline_interpolation() {
    let src = "\"\"\"a${b}c\"\"\"";
    assert_kinds(
        &scan_src(src),
        &[
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::CloseCurly,
            K::String,
        ],
    );
}

#[test]
fn adjacent_strings_concat() {
    // Adjacent strings (Dart implicit concatenation) are separate tokens.
    assert_kinds(&scan_src("'a' 'b'"), &[K::String, K::String]);
}

#[test]
fn dollar_in_code_is_identifier_start() {
    // In code, `$` starts an identifier (official bigSwitch $$ branch).
    assert_kinds(
        &scan_src("var $x = 1;"),
        &[
            K::Keyword(Keyword::Var),
            K::Identifier,
            K::Eq,
            K::Int,
            K::Semicolon,
        ],
    );
}

#[test]
fn interpolation_official_corpus_ranges() {
    // Exact byte ranges from the official corpus
    // (front_end/test/scanner_test.dart:1034-1051): the prefix STRING ends
    // BEFORE the `$` (`"Hello "` [0..7) with `${`/`$` at 7).
    let src = "\"Hello ${name}!\"";
    let r = scan_src(src);
    let t = &r.tokens;
    assert_eq!(t.lexeme(src, 0), "\"Hello ");
    assert_eq!(t.get(0).unwrap().byte_range(), 0..7);
    assert_eq!(t.lexeme(src, 1), "${");
    assert_eq!(t.get(1).unwrap().byte_offset, 7);
    assert_eq!(t.lexeme(src, 2), "name");
    assert_eq!(t.get(2).unwrap().byte_offset, 9);
    assert_eq!(t.get(3).unwrap().byte_offset, 13); // }
    assert_eq!(t.lexeme(src, 4), "!\"");
    assert_eq!(t.get(4).unwrap().byte_offset, 14);
    let src = "\"Hello $name!\"";
    let r = scan_src(src);
    let t = &r.tokens;
    assert_eq!(t.get(0).unwrap().byte_range(), 0..7);
    assert_eq!(t.lexeme(src, 1), "$");
    assert_eq!(t.get(1).unwrap().byte_offset, 7);
    assert_eq!(t.get(2).unwrap().byte_offset, 8); // name
    assert_eq!(t.lexeme(src, 3), "!\"");
    assert_eq!(t.get(3).unwrap().byte_offset, 12);
}

#[test]
fn synthetic_tokens_dual_coordinate_identity() {
    // PRD §10.4 identity must hold for synthetic tokens too: utf16_offset is
    // derived from byte_offset (F3: the synthetic `0` of `0x`/`1e` starts at
    // the number start while the cursor sits past the missing digits — a
    // lexeme-based derivation drifted by the skipped suffix, growing with the
    // number length).
    let sources = [
        "0x",
        "0x 1e",
        "1e",
        "1e+",
        "123.456e-",
        "1_000.5e",
        "0XFF + 12.5e",
        "var x = 0x; var y = 1e;",
        "'\u{1F600}' 0x",    // non-BMP prefix: 4 bytes / 2 utf16 units
        "你好 + 0x + 1234e", // multi-byte prefix
        "f('\u{1F600}', 0x, 1234e)",
        "'a${0x}'",
    ];
    for src in sources {
        let r = scan_src(src);
        for t in r.tokens.as_slice() {
            assert_eq!(
                byte_offset_to_utf16(src, t.byte_offset as usize),
                t.utf16_offset,
                "src={src:?} token kind={:?} byte_offset={}",
                t.kind,
                t.byte_offset
            );
        }
    }
    // Spot-check the corrected values: the synthetic `0` of `123.456e-` pins
    // to the number start (utf16 0), not drifted by the 8 skipped units.
    let src = "123.456e-";
    let r = scan_src(src);
    let first = r.tokens.first();
    assert!(first.is_synthetic());
    assert_eq!(first.utf16_offset, 0);
    assert_eq!(first.byte_offset, 0);
}

#[test]
fn raw_multiline_dollar_is_content() {
    // Raw strings never interpolate: `$` stays content (official
    // tokenizeMultiLineRawString has no `$` branch).
    let src = "r'''a$b${c}'''";
    let r = scan_src(src);
    assert_kinds(&r, &[K::String]);
    assert_eq!(r.tokens.first().byte_range(), 0..src.len());
    assert!(r.diagnostics.is_empty());
    let src = "r\"\"\"${x}\"\"\"";
    let r = scan_src(src);
    assert_kinds(&r, &[K::String]);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn adjacent_strings_inside_interpolation() {
    // Inside `${}` the scanner is back in code mode: adjacent string literals
    // are separate STRING segments (official tokenizeStringInterpolation →
    // $STX main-loop reentry).
    let src = "'a${'b' 'c'}d'";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::String,                        // 'a
            K::StringInterpolationExpression, // ${
            K::String,                        // 'b'
            K::String,                        // 'c'
            K::CloseCurly,                    // }
            K::String,                        // d'
        ],
    );
    assert!(r.diagnostics.is_empty());
}
