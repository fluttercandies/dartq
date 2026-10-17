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
    assert_eq!(
        r.diagnostics[0].message,
        "Digit separators ('_') in a number literal can only be placed between two digits."
    );
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
    assert_eq!(r.diagnostics[0].message, "Hexadecimal digit expected.");
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
    assert_eq!(r.diagnostics[0].message, "Decimal digit expected.");
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
    // Synthetic STRING segment spanning the remaining content from the string
    // start (official SyntheticStringToken: [0, 4), closing quote in the
    // lexeme but not the length) + unterminated_string_literal (pinned to
    // the last character `c`).
    assert_eq!(
        r.tokens
            .as_slice()
            .iter()
            .filter(|t| t.kind == K::String)
            .count(),
        1
    );
    assert!(r.tokens.first().is_synthetic());
    assert_eq!(r.tokens.first().byte_range(), 0..4);
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 3);
    assert_eq!(r.diagnostics[0].range.length, 1);
    assert_eq!(r.diagnostics[0].message, "Unterminated string literal.");
    // EOL terminates scanning; the second line's `c` and the lone `'` are separate tokens — the latter is also unterminated.
    let r = scan_src("'ab\nc'");
    assert_codes(
        &r,
        &["unterminated_string_literal", "unterminated_string_literal"],
    );
    assert_kinds(&r, &[K::String, K::Identifier, K::String]);
    // The first segment carries the real content up to the break (`'ab`);
    // the second is empty (EOF cascade: upstream `start = scanOffset`).
    assert_eq!(r.tokens.get(0).unwrap().byte_range(), 0..3);
    assert_eq!(r.tokens.get(2).unwrap().byte_range(), 5..6);
}

#[test]
fn escaped_newline_terminates_single_line() {
    // The official scanner checks LF/CR after the backslash advance
    // (abstract_scanner.dart tokenizeSingleLineString): `'a\` + EOL does NOT
    // continue the string. The part's content keeps the backslash, so unescape
    // adds invalidEscapeStarted ("can't stand alone") to the unterminated
    // string; both pin to the backslash (last unit before the EOL). After the
    // EOL `b` scans as code and the dangling `'` is a second unterminated
    // string (same shape as the plain-EOL case).
    let r = scan_src("'a\\\nb'");
    assert_codes(
        &r,
        &[
            "invalid_unicode_escape_started",
            "unterminated_string_literal",
            "unterminated_string_literal",
        ],
    );
    assert_kinds(&r, &[K::String, K::Identifier, K::String]);
    assert!(r.tokens.first().is_synthetic());
    assert_eq!(r.diagnostics[0].range.offset, 2);
    assert_eq!(r.diagnostics[0].range.length, 1);
    assert_eq!(
        r.diagnostics[0].message,
        "The string '\\' can't stand alone."
    );
    assert_eq!(r.diagnostics[1].range.offset, 2);
    assert_eq!(r.diagnostics[1].range.length, 1);
    // A CR line break terminates the same way.
    let r = scan_src("'a\\\rb'");
    assert_codes(
        &r,
        &[
            "invalid_unicode_escape_started",
            "unterminated_string_literal",
            "unterminated_string_literal",
        ],
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
fn unterminated_multiline_at_eof_segment_shape() {
    // Corpus string3.dart (regression f59c79c8): `"""\næbler` truncated at
    // EOF — the synthetic STRING segment spans from the opening quote through
    // the last real content unit (official SyntheticStringToken: offset =
    // tokenStart, length = content length without the synthetic suffix), and
    // the diagnostic pins to the last UTF-16 unit (analyzer
    // `endOffset - 1, length 1`, translate_error_token.dart; the corpus
    // oracle's unterminated_string_literal row lands on the `r`).
    let src = "  return \"\"\"\næbler";
    let r = scan_src(src);
    assert_codes(&r, &["unterminated_string_literal"]);
    let t = &r.tokens;
    assert_eq!(t.get(1).unwrap().kind, K::String);
    assert!(t.get(1).unwrap().is_synthetic());
    assert_eq!(t.get(1).unwrap().byte_range(), 9..19); // `"""` through `æbler` (æ is 2 bytes)
    assert_eq!(t.get(1).unwrap().utf16_offset, 9);
    assert_eq!(r.diagnostics[0].range.offset, 17); // last unit of `æbler`
    assert_eq!(r.diagnostics[0].range.length, 1);
}

#[test]
fn unterminated_multiline_raw_at_eof_segment_shape() {
    // Corpus string4.dart (regression e0f42062): `r"""` truncated at EOF —
    // the synthetic segment's tokenStart includes the `r` prefix (official
    // `unterminatedString` passes the literal's `start`, which is the raw
    // marker for a raw string).
    let src = "  return r\"\"\"\næbler";
    let r = scan_src(src);
    assert_codes(&r, &["unterminated_string_literal"]);
    let t = &r.tokens;
    assert_eq!(t.get(1).unwrap().kind, K::String);
    assert!(t.get(1).unwrap().is_synthetic());
    assert_eq!(t.get(1).unwrap().byte_range(), 9..20); // `r"""` through `æbler`
    assert_eq!(t.get(1).unwrap().utf16_offset, 9);
    assert_eq!(r.diagnostics[0].range.offset, 18); // last unit of `æbler`
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
fn empty_interp_close_paren_faces() {
    // #935, SII probe family (f-'${ insertion postures, #921): a `}` closing
    // an interpolation is followed by string-body scanning, so an immediate
    // closing quote ends the string — `f('${}')` scans with zero diagnostics
    // (oracle dart 3.13.1: no scanner row; the #921 lane's static prediction
    // of an unterminated_string_literal here was wrong, the replay overwrites
    // it). A quote-less tail stays unterminated, pinned at the last content
    // character before the break (the analyzer's `endOffset - 1` rule).
    let src = "f('${}')";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::CloseCurly,
            K::String,
            K::CloseParen,
        ],
    );
    // Token table: 0 `f`, 1 `(`, 2 `'` (prefix segment), 3 `${`, 4 `}`,
    // 5 `'` (tail segment), 6 `)`. The `${` pairs with its `}` (idx 4) —
    // `close_matched` records the emitted closer's index, matching the
    // sibling pins below ("outer ${ → the final }"). The originally pinned
    // `Some(5)` pointed at the tail String segment, contradicting the kinds
    // pin above (CloseCurly at idx 4); the scanner emits no token between
    // `${` and `}` for an empty expression (oracle: no scanner row for
    // `f('${}')` — the interpolation is scanner-well-formed).
    let t = &r.tokens;
    assert_eq!(t.lexeme(src, 2), "'");
    assert_eq!(t.lexeme(src, 3), "${");
    assert_eq!(t.matching_end(3), Some(4));
    assert_eq!(t.lexeme(src, 5), "'");
    assert_eq!(t.lexeme(src, 6), ")");

    // Quote-less tail inside the call: `)` and `;` stay string content; the
    // pin lands on the `;` (oracle `f('${});` row 1|8|1). The `(` never sees
    // a real `)` — this one is string content — so the unmatched begin gets
    // its eof-synthetic closer: the pair is recorded on the table and the
    // parser reports the row post-parse (same unmatched-begin-at-eof law as
    // `f({` and `'a${b`; the `${` itself matched the real `}`, no pair).
    let r = scan_src("f('${});");
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 7);
    assert_eq!(r.diagnostics[0].range.length, 1);
    let t = &r.tokens;
    let n = t.as_slice().len();
    assert!(t.get(n - 2).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(1, (n - 2) as u32)]);

    // No enclosing call: same rule, pin on the `)` (oracle `var s = '${})`
    // row 1|13|1).
    let r = scan_src("var s = '${})");
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 12);
    assert_eq!(r.diagnostics[0].range.length, 1);

    // `)` inside the empty interpolation deep-matches the enclosing `(`: the
    // `${` is recorded unmatched at the `)` (insert posture; the parser
    // reports the row post-parse), then the body resumes and hits the break
    // with an empty tail. The endOffset - 1 pin falls on the `)` — the last
    // string-facing character; a length-1 range at offset 6 would sit past
    // the 6-byte source (eof-adjacent diagnostics are zero-width at eof,
    // never length-1 past the end).
    let r = scan_src("f('${)");
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 5);
    assert_eq!(r.diagnostics[0].range.length, 1);
    // 0 f, 1 (, 2 ', 3 ${, 4 synthetic }, 5 ). The `${` pairs with the
    // synthetic; the `(` matched the real `)`.
    let t = &r.tokens;
    assert!(t.get(4).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(3, 4)]);

    // EOF inside the interpolation: unterminated pinned at the break, then
    // the eof-cascade empty segment and the eof-synthetic closers — the open
    // `${` AND the enclosing `(` (same unmatched-begin-at-eof law as `f({`):
    // both pairs recorded, both rows reported by the parser post-parse. The
    // string consumed nothing after `${`, so the tail segment is the empty
    // [5,5) (upstream appendSyntheticSubstringToken with start = scanOffset =
    // eof) and endOffset - 1 drops the unterminated pin onto the `{` (a pin
    // at offset 5 would be a length-1 range past the 5-byte source).
    let r = scan_src("g('${");
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 4);
    assert_eq!(r.diagnostics[0].range.length, 1);
    // 0 g, 1 (, 2 String(`'`), 3 `${`, 4 empty eof-cascade segment, 5
    // synthetic `}`, 6 synthetic `)`, 7 eof — the tail segment precedes the
    // closers (upstream token order: unterminatedString's substring token,
    // then appendEofToken's unmatchedBeginGroup closers); `${` pops first
    // (stack top), then the `(`.
    let t = &r.tokens;
    assert!(t.get(4).unwrap().is_synthetic());
    assert_eq!(t.get(4).unwrap().byte_range(), 5..5);
    assert!(t.get(5).unwrap().is_synthetic());
    assert!(t.get(6).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(3, 5), (1, 6)]);
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
    // identity must hold for synthetic tokens too: utf16_offset is
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

#[test]
fn nested_unterminated_diagnostic_order() {
    // Two-level unterminated nesting: the inner `"` string's recovery and the
    // outer `'` string's EOF-cascade recovery both pin one unit before EOF
    // with the identical (code, offset, length, message) identity, and the
    // oracle's row set is a Set over that identity — the pair surfaces once
    // (the same collapse the `f('${f('${')` capture shows: two rows over
    // three error tokens). Identical rows are unobservable in any order by
    // construction — the dedup structural invariant (tests/dedup.rs) keeps
    // them adjacent so the finish pass removes all of them.
    let src = "'a${\"b${c";
    let r = scan_src(src);
    assert_codes(&r, &["unterminated_string_literal"]);
    // The collapse keeps the shared pin: one unit before EOF (the last
    // character `c`), registry-true message.
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 8);
    assert_eq!(d.range.length, 1);
    assert_eq!(d.message, "Unterminated string literal.");
    // Token tail: the two unterminated synthetics, then the closers LIFO
    // (inner SII → the first `}`), then Eof.
    let slice = r.tokens.as_slice();
    let n = slice.len();
    assert_eq!(slice[n - 3].kind, K::CloseCurly);
    assert!(slice[n - 3].is_synthetic());
    assert_eq!(slice[n - 2].kind, K::CloseCurly);
    assert!(slice[n - 2].is_synthetic());
    assert_eq!(slice[n - 1].kind, K::Eof);
    assert_eq!(r.tokens.matching_end(3), Some(n - 3)); // inner ${ → inner }
    assert_eq!(r.tokens.matching_end(1), Some(n - 2)); // outer ${ → outer }
    // Both eof-parked closers, inner first: the parser reports the two
    // expected_token rows in this order, zero-width at the eof position.
    assert_eq!(
        r.tokens.unmatched_groups(),
        &[(3, (n - 3) as u32), (1, (n - 2) as u32)]
    );
}

#[test]
fn sii_probe_dollar_eof_unterminated_pins() {
    // The sii-probe `$`-EOF family (oracle `dart analyze --format=machine`
    // rows), pinned at scanner level: the driver's EOF unwind emits one
    // unterminated recovery per suspended body with the `endOffset - 1` pin,
    // and the `$` identifier form's missing_identifier rides alongside.
    //
    // bare_dollar (`'$\n`): the `$` takes the identifier form with the break
    // as the "identifier" — missing_identifier pins past the `$` (col 3),
    // then the body's line termination reports the unterminated string one
    // unit back (col 2). Emission order is the finish pass's stable sort:
    // both records start at the break.
    let r = scan_src("'$\n");
    assert_codes(&r, &["missing_identifier", "unterminated_string_literal"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 2);
    assert_eq!(d.range.length, 1);
    assert_eq!(d.message, "Expected an identifier.");
    let d = &r.diagnostics[1];
    assert_eq!(d.range.offset, 1);
    assert_eq!(d.range.length, 1);
    assert_eq!(d.message, "Unterminated string literal.");

    // double_dollar (`f('$${')\n`): the first `$` takes the identifier form
    // (`$` itself is not an identifier start → missing_identifier @col 5);
    // the second `$` opens a `${` whose string rides to the break and EOF —
    // two unterminated rows, oracle 1|8|1 and 1|9|1. The `(` and the open
    // `${` close with eof-synthetic closers recorded as unmatched pairs (the
    // parser reports their rows post-parse).
    let r = scan_src("f('$${')\n");
    assert_codes(
        &r,
        &[
            "missing_identifier",
            "unterminated_string_literal",
            "unterminated_string_literal",
        ],
    );
    assert_eq!(r.diagnostics[0].range.offset, 4);
    assert_eq!(r.diagnostics[0].range.length, 1);
    assert_eq!(r.diagnostics[0].message, "Expected an identifier.");
    for d in &r.diagnostics[1..] {
        assert_eq!(d.message, "Unterminated string literal.");
        assert_eq!(d.range.length, 1);
    }
    assert_eq!(r.diagnostics[1].range.offset, 7); // the break (col 8)
    assert_eq!(r.diagnostics[2].range.offset, 8); // eof - 1 (col 9)
    let pairs = r.tokens.unmatched_groups();
    assert_eq!(pairs.len(), 2, "the paren and SIE groups: {pairs:?}");
    for &(_, close) in pairs {
        assert!(r.tokens.get(close as usize).unwrap().is_synthetic());
    }

    // bare_interp (`'${\n`): EOF right after the `${` — the unterminated pin
    // falls on the break (col 4, oracle 1:4), and the eof-synthetic `}`
    // closer rides the pair list (the `'a${b` law; zero-width at eof when
    // the parser reports it).
    let r = scan_src("'${\n");
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 3);
    assert_eq!(r.diagnostics[0].range.length, 1);
    assert_eq!(r.tokens.unmatched_groups().len(), 1);

    // two_unclosed_curly (`f('${{\n`): the `{` after `${` stays an ordinary
    // code-mode begin group; only the string reports unterminated (oracle
    // 1:7 = eof - 1), and all three open groups (`(`, `${`, `{`) close with
    // eof-synthetic closers recorded as pairs behind it.
    let r = scan_src("f('${{\n");
    assert_codes(&r, &["unterminated_string_literal"]);
    assert_eq!(r.diagnostics[0].range.offset, 6);
    assert_eq!(r.diagnostics[0].range.length, 1);
    let pairs = r.tokens.unmatched_groups();
    assert_eq!(pairs.len(), 3, "paren, SIE and curly groups: {pairs:?}");
    for &(_, close) in pairs {
        assert!(r.tokens.get(close as usize).unwrap().is_synthetic());
    }
}

#[test]
fn deep_nested_unterminated_interpolation() {
    // 50_000 levels of `"a${` hitting EOF: the interpolation frames live on
    // the heap, so the scan completes where a native-stack-recursive scanner
    // would overflow (SIGSEGV). Every unwound recovery pins the identical
    // (code, eof - 1, 1, message) row, so the oracle Set semantics collapse
    // them all into one (the finish-pass dedup; the row cap never engages).
    let src = "\"a${".repeat(50_000);
    let r = scan_src(&src);
    assert_eq!(r.diagnostics.len(), 1);
    let d = &r.diagnostics[0];
    assert_eq!(d.code.name, "unterminated_string_literal");
    assert_eq!(d.range.offset, u32::try_from(src.len() - 1).unwrap());
    assert_eq!(d.range.length, 1);
    let eof = r.tokens.get(r.tokens.len() - 1).unwrap();
    assert_eq!(eof.kind, K::Eof);
}

#[test]
fn deep_nested_wellformed_interpolation() {
    // 1_000 balanced levels: `"a${` ×1000 + `a` + `}"` ×1000 — zero
    // diagnostics, flat token stream, Eof sentinel last.
    let mut src = "\"a${".repeat(1_000);
    src.push('a');
    src.push_str(&"}\"".repeat(1_000));
    let r = scan_src(&src);
    assert!(r.diagnostics.is_empty());
    let slice = r.tokens.as_slice();
    // Per level: prefix STRING + `${` + `}` + suffix STRING; plus the
    // innermost identifier and Eof.
    assert_eq!(slice.len(), 1_000 * 4 + 2);
    assert_eq!(slice.last().unwrap().kind, K::Eof);
    // The outermost `${` closes at the last `}` (followed only by the
    // outermost suffix segment and Eof).
    assert_eq!(slice[slice.len() - 2].kind, K::String);
    assert_eq!(slice[slice.len() - 3].kind, K::CloseCurly);
    assert_eq!(r.tokens.matching_end(1), Some(slice.len() - 3));
}
