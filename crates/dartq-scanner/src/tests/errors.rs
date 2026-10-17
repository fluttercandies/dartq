//! Recovery strategies, diagnostic pinning, the @dart window, CRLF, BOM-as-content, dual-offset consistency.

use super::{assert_codes, assert_kinds, codes, scan_src};
use crate::{Keyword, TokenKind as K};
use dartq_source::byte_offset_to_utf16;

#[test]
fn nested_block_comments() {
    let r = scan_src("a /* /* nested */ still */ b");
    assert_kinds(&r, &[K::Identifier, K::Identifier]);
    assert!(r.diagnostics.is_empty());
    // Nesting depth > 2.
    assert_kinds(&scan_src("/* 1 /* 2 /* 3 */ 2 */ 1 */ x"), &[K::Identifier]);
}

#[test]
fn unterminated_block_comment() {
    let r = scan_src("a /* oops");
    assert_kinds(&r, &[K::Identifier]);
    assert_codes(&r, &["unterminated_multi_line_comment"]);
    // Pinned to the last character (EOF-1).
    let src = "a /* oops";
    assert_eq!(r.diagnostics[0].range.offset as usize, src.len() - 1);
    assert_eq!(r.diagnostics[0].range.length, 1);
}

#[test]
fn line_comments_skipped() {
    let r = scan_src("a // rest of line\nb // another");
    assert_kinds(&r, &[K::Identifier, K::Identifier]);
    assert!(r.diagnostics.is_empty());
    // `///` and `/**` doc comments also produce no tokens (official precedingComments shape).
    assert_kinds(
        &scan_src("/// doc\nvoid f() {}"),
        &[
            K::Keyword(Keyword::Void),
            K::Identifier,
            K::OpenParen,
            K::CloseParen,
            K::OpenCurly,
            K::CloseCurly,
        ],
    );
    assert_kinds(&scan_src("/** doc */ x"), &[K::Identifier]);
}

#[test]
fn bom_is_content() {
    // The engine/CLI decode faces strip a leading BOM, so a U+FEFF the
    // scanner sees is ordinary content: a non-ASCII space character
    // diagnostic (official error_token.dart case 0xFEFF), never a silent
    // skip. Offsets stay natural — the oracle reports the same numbers for a
    // double-BOM file (its read layer strips exactly one BOM).
    let src = "\u{FEFF}var x;";
    let r = scan_src(src);
    assert_kinds(&r, &[K::Keyword(Keyword::Var), K::Identifier, K::Semicolon]);
    assert_codes(&r, &["illegal_character"]);
    assert_eq!(r.diagnostics[0].range.offset, 0);
    assert_eq!(r.diagnostics[0].range.length, 1);
    let t = r.tokens.first();
    assert_eq!(t.byte_offset, 3);
    assert_eq!(t.utf16_offset, 1);
    assert_eq!(byte_offset_to_utf16(src, 3), 1);
    // A content BOM also blocks script-tag recognition (`#!` is no longer
    // the file's first byte sequence).
    let r = scan_src("\u{FEFF}#!/bin/dart");
    assert_codes(&r, &["illegal_character"]);
    assert!(!r.tokens.as_slice().iter().any(|t| t.kind == K::ScriptTag));
}

#[test]
fn crlf_and_lone_cr() {
    // CRLF and LF both terminate line scanning; a lone CR acts as whitespace (official bigSwitch).
    for sep in ["\r\n", "\n", "\r"] {
        let r = scan_src(&format!("a{sep}b"));
        assert_kinds(&r, &[K::Identifier, K::Identifier]);
        assert!(r.diagnostics.is_empty());
    }
    // CR ends a line comment.
    let r = scan_src("a // c\rb");
    assert_kinds(&r, &[K::Identifier, K::Identifier]);
}

#[test]
fn line_index_authority() {
    let src = "var a;\nvar b;\n";
    let r = scan_src(src);
    assert_eq!(r.line_index.line_count(), 3); // trailing newline → final empty line
    let (line, _col) = r.line_index.byte_to_line_col(7);
    assert_eq!(line, 2);
}

#[test]
fn illegal_control_character() {
    let r = scan_src("a \u{1} b");
    assert_kinds(&r, &[K::Identifier, K::Identifier]);
    assert_codes(&r, &["illegal_character"]);
    assert_eq!(r.diagnostics[0].range.offset, 2);
    assert_eq!(r.diagnostics[0].range.length, 1);
}

#[test]
fn illegal_non_ascii_identifier_recovery() {
    // `var 한글`: one illegal_character per non-ASCII character; official recovery
    // merges the consecutive illegal characters with the adjacent identifier into
    // a **single** Identifier (NonAsciiIdentifierToken recovery: when the tail is
    // an adjacent IDENTIFIER it is folded in and rebuilt).
    let src = "var 한글;";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::Keyword(Keyword::Var),
            K::Identifier, // the non-ASCII chars (merged recovery)
            K::Semicolon,
        ],
    );
    assert_codes(&r, &["illegal_character", "illegal_character"]);
    let t = &r.tokens;
    assert_eq!(t.lexeme(src, 1), "한글");
    assert_eq!(t.get(1).unwrap().byte_range(), 4..10); // 3B + 3B
}

#[test]
fn illegal_char_merges_with_adjacent_identifier() {
    // Official recovery shape: the illegal character merges with the **preceding adjacent** identifier into a single token.
    let src = "ab¢cd";
    let r = scan_src(src);
    assert_kinds(&r, &[K::Identifier]);
    assert_codes(&r, &["illegal_character"]);
    let t = r.tokens.first();
    assert_eq!(t.byte_range(), 0..src.len());
    // Dual-offset consistency: utf16 of byte 5 (start of 'd') = 4 (¢ takes 2 bytes / 1 unit).
    assert_eq!(t.utf16_offset, 0);
    assert_eq!(byte_offset_to_utf16(src, 5), 4);
}

#[test]
fn illegal_astral_char_double_emission() {
    // An astral illegal character is reported **once per UTF-16 surrogate**
    // (official UTF-16 scanners see each code unit separately): oracle
    // `dart analyze --format=json` on `var x = 1;\n😀\n` yields
    // illegal_character @11 len1 char 55357 and @12 len1 char 56832 —
    // the two surrogate code points, one unit each.
    let src = "var x = 1;\n😀\n"; // emoji bytes 11..15, utf16 units 11..13
    let r = scan_src(src);
    assert_codes(&r, &["illegal_character", "illegal_character"]);
    let d0 = &r.diagnostics[0];
    let d1 = &r.diagnostics[1];
    assert_eq!(d0.range.offset, 11); // high surrogate
    assert_eq!(d0.range.length, 1);
    assert!(d0.message.contains("55357"), "{}", d0.message);
    assert_eq!(d1.range.offset, 12); // low surrogate
    assert_eq!(d1.range.length, 1);
    assert!(d1.message.contains("56832"), "{}", d1.message);
    // Mid-file form (oracle `var x = 1;\n😀\nvar y = 2;\n`): same two pins,
    // nothing else from the scanner.
    let r = scan_src("var x = 1;\n😀\nvar y = 2;\n");
    assert_codes(&r, &["illegal_character", "illegal_character"]);
    assert_eq!(r.diagnostics[0].range.offset, 11);
    assert_eq!(r.diagnostics[1].range.offset, 12);
    // The character itself still merges into one identifier token (recovery
    // shape unchanged — only the diagnostic count differs).
    let r = scan_src("x😀y");
    assert_kinds(&r, &[K::Identifier]);
    assert_codes(&r, &["illegal_character", "illegal_character"]);
}

#[test]
fn illegal_character_registry_message() {
    // One registry template for every token class: `Illegal character '{0}'.`
    // with `{0}` the decimal UTF-16 code unit (analyzer messages.yaml
    // `illegalCharacter`, parameter `int codePoint`). Exact pins against the
    // captured oracle rows of the known-diffs GAP-SPIN trio:
    // `a = Щ;` → `Illegal character '1065'.` @1:5 (dbf4285a),
    // `a = ShcheгlovЩx;` → `'1075'.` @1:10 + `'1065'.` @1:14 (4caf0953),
    // the astral emoji → `'55357'.` + `'56832'.` one unit each (8f05c31e,
    // line 5 there; the relative pins here are line-1 equivalents).
    let r = scan_src("a = Щ;");
    assert_eq!(r.diagnostics[0].message, "Illegal character '1065'.");
    assert_eq!(r.diagnostics[0].range.offset, 4);
    let r = scan_src("a = ShcheгlovЩx;");
    assert_eq!(r.diagnostics[0].message, "Illegal character '1075'.");
    assert_eq!(r.diagnostics[0].range.offset, 9);
    assert_eq!(r.diagnostics[1].message, "Illegal character '1065'.");
    assert_eq!(r.diagnostics[1].range.offset, 13);
    let r = scan_src("f(😀);");
    assert_eq!(r.diagnostics[0].message, "Illegal character '55357'.");
    assert_eq!(r.diagnostics[0].range.offset, 2);
    assert_eq!(r.diagnostics[1].message, "Illegal character '56832'.");
    assert_eq!(r.diagnostics[1].range.offset, 3);
    // The remaining token classes share the template (registry-derived; the
    // live-oracle captures above cover the identifier and surrogate faces).
    let r = scan_src("a \u{1} b"); // ASCII control
    assert_eq!(r.diagnostics[0].message, "Illegal character '1'.");
    let r = scan_src("ab\u{7F}cd"); // DEL (default arm → merge path)
    assert_eq!(r.diagnostics[0].message, "Illegal character '127'.");
    let r = scan_src("\u{FEFF}var x;"); // non-ASCII whitespace class
    assert_eq!(r.diagnostics[0].message, "Illegal character '65279'.");
}

#[test]
fn illegal_pure_junk_is_an_identifier() {
    // A junk run with no adjacent ASCII identifier still becomes a real
    // Identifier token (official `unexpected`, abstract_scanner.dart:2470:
    // NonAsciiIdentifierToken appends IDENTIFIER over the collected code units;
    // with no adjacent tail the token starts at the junk itself).
    // Oracle `a = Щ;` (dart 3.13.1 --format=machine):
    //   ILLEGAL_CHARACTER @1:5 len1 '1065'; UNDEFINED_IDENTIFIER @1:5 len1 'Щ'.
    let r = scan_src("a = Щ;");
    assert_kinds(&r, &[K::Identifier, K::Eq, K::Identifier, K::Semicolon]);
    assert_codes(&r, &["illegal_character"]);
    let t = &r.tokens;
    assert_eq!(t.lexeme("a = Щ;", 2), "Щ");
    assert_eq!(t.get(2).unwrap().byte_range(), 4..6);
    assert_eq!(t.get(2).unwrap().utf16_offset, 4);
    assert_eq!(r.diagnostics[0].range.offset, 4);
    assert_eq!(r.diagnostics[0].range.length, 1);
    // Bare junk at file origin (`Щ`): same single-token shape at offset 0.
    let r = scan_src("Щ");
    assert_kinds(&r, &[K::Identifier]);
    assert_codes(&r, &["illegal_character"]);
    assert_eq!(r.tokens.first().byte_range(), 0..2);
    assert_eq!(r.tokens.first().utf16_offset, 0);
}

#[test]
fn illegal_run_merges_into_one_identifier() {
    // Mixed ASCII + junk run: each junk character folds the partially built
    // identifier back in (tail-merge, abstract_scanner.dart:2476-2481), so
    // `a = ShcheгlovЩx;` scans to ONE Identifier spanning the whole run.
    // Oracle `a = ShcheгlovЩx;` (dart 3.13.1 --format=machine):
    //   UNDEFINED_IDENTIFIER @1:5 len11 'ShcheгlovЩx'
    //   ILLEGAL_CHARACTER @1:10 len1 '1075' (г) and @1:14 len1 '1065' (Щ)
    // — i.e. utf16 anchors 9 and 13, token utf16 span 4..15.
    let src = "a = ShcheгlovЩx;";
    let r = scan_src(src);
    assert_kinds(&r, &[K::Identifier, K::Eq, K::Identifier, K::Semicolon]);
    assert_codes(&r, &["illegal_character", "illegal_character"]);
    let t = &r.tokens;
    assert_eq!(t.lexeme(src, 2), "ShcheгlovЩx");
    assert_eq!(t.get(2).unwrap().byte_range(), 4..17); // 5+2+3+2+1 bytes
    assert_eq!(t.get(2).unwrap().utf16_offset, 4);
    assert_eq!(byte_offset_to_utf16(src, 17), 15); // token utf16 end: 4+11
    assert_eq!(r.diagnostics[0].range.offset, 9); // г
    assert_eq!(r.diagnostics[0].range.length, 1);
    assert_eq!(r.diagnostics[1].range.offset, 13); // Щ (utf16: 9 units + г)
    assert_eq!(r.diagnostics[1].range.length, 1);
}

#[test]
fn illegal_astral_char_token_span() {
    // The astral junk run is one Identifier covering the whole character
    // (official UTF-16 scanners see each surrogate hit `unexpected` separately;
    // the low surrogate tail-merges into the high surrogate's token), so the
    // downstream `Undefined name '😀'` face spans the surrogate pair.
    // Oracle `f(😀);` (dart 3.13.1 --format=machine, line-scoped cols):
    //   ILLEGAL_CHARACTER col5 len1 '55357'; UNDEFINED_IDENTIFIER col5 len2 '😀';
    //   ILLEGAL_CHARACTER col6 len1 '56832'.
    let src = "f(😀);";
    let r = scan_src(src);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::Identifier,
            K::CloseParen,
            K::Semicolon,
        ],
    );
    assert_codes(&r, &["illegal_character", "illegal_character"]);
    let t = &r.tokens;
    assert_eq!(t.lexeme(src, 2), "😀");
    assert_eq!(t.get(2).unwrap().byte_range(), 2..6); // 4-byte character
    assert_eq!(t.get(2).unwrap().utf16_offset, 2);
    assert_eq!(byte_offset_to_utf16(src, 6), 4); // utf16 span 2..4 (len 2)
    // Surrogate anchors: high @2, low @3 (one unit each).
    assert_eq!(r.diagnostics[0].range.offset, 2);
    assert_eq!(r.diagnostics[0].range.length, 1);
    assert!(r.diagnostics[0].message.contains("55357"));
    assert_eq!(r.diagnostics[1].range.offset, 3);
    assert_eq!(r.diagnostics[1].range.length, 1);
    assert!(r.diagnostics[1].message.contains("56832"));
}

#[test]
fn unicode_whitespace_skipped() {
    // Non-ASCII whitespace like U+00A0: illegal_character + skip (official NonAsciiWhitespace).
    let r = scan_src("a\u{00A0}b");
    assert_kinds(&r, &[K::Identifier, K::Identifier]);
    assert_codes(&r, &["illegal_character"]);
}

#[test]
fn dual_offset_consistency_non_ascii() {
    // Non-BMP character: utf16_offset correctly derived from byte_offset (emoji takes 4 bytes / 2 units).
    let src = "'😀' x"; // ' + 4byte emoji + ' = 6 bytes, then space, x
    let r = scan_src(src);
    let x = r
        .tokens
        .as_slice()
        .iter()
        .find(|t| t.kind == K::Identifier)
        .unwrap();
    assert_eq!(x.byte_offset, 7);
    assert_eq!(x.utf16_offset, 5); // 2(quotes) + 2(surrogate pair) + 1(space)
    assert_eq!(byte_offset_to_utf16(src, 7), 5);
    // Full-file utf16 length conversion.
    assert_eq!(byte_offset_to_utf16(src, src.len()), 6);
}

#[test]
fn diag_ranges_utf16_with_non_ascii() {
    // With a non-ASCII prefix, diagnostic ranges are still UTF-16.
    let src = "'😀' 'abc"; // 8 bytes prefix, unterminated string starts at byte 7
    let r = scan_src(src);
    assert_codes(&r, &["unterminated_string_literal"]);
    // Pinned to the last character `c`: byte 10 → utf16 8 (😀 takes 4 bytes / 2 units).
    assert_eq!(r.diagnostics[0].range.offset, 8);
}

#[test]
fn unclosed_groups_at_eof() {
    // `f({` → two synthetic close tokens: the stack top (inner `{`) pops first →
    // `}` before `)` (official unmatchedBeginGroup linear order: syn_inner … syn_outer).
    // The pairs are recorded on the table; the parser reports the rows
    // post-parse (official UnmatchedToken translation is post-parse).
    let r = scan_src("f({");
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let slice = r.tokens.as_slice();
    let n = slice.len();
    assert_eq!(slice[n - 3].kind, K::CloseCurly);
    assert!(slice[n - 3].is_synthetic());
    assert_eq!(slice[n - 2].kind, K::CloseParen);
    assert!(slice[n - 2].is_synthetic());
    // begin → synthetic close token pairing (official endGroup semantics).
    assert_eq!(r.tokens.matching_end(2), Some(n - 3)); // `{` → `}`
    assert_eq!(r.tokens.matching_end(1), Some(n - 2)); // `(` → `)`
    // The unmatched pairs, in scan-close order (inner `{` first).
    assert_eq!(
        r.tokens.unmatched_groups(),
        &[(2, (n - 3) as u32), (1, (n - 2) as u32)]
    );
}

#[test]
fn unclosed_interpolation_at_eof() {
    // `'a${b` → unclosed `${`: the pair rides the table (the parser reports
    // the `expected_token` row post-parse); the scanner's own diagnostic is
    // just the unterminated string.
    let r = scan_src("'a${b");
    assert_codes(&r, &["unterminated_string_literal"]);
    // The unterminated-string pin keeps the `endOffset - 1` law (last
    // character `b` → [4,5)); the `}` row reports post-parse at
    // `begin.endGroup` (the eof-parked synthetic).
    assert_eq!(r.diagnostics[0].range.offset, 4);
    assert_eq!(r.diagnostics[0].range.length, 1);
    // 0 String(`'a`), 1 `${`, 2 b, 3 empty eof-cascade segment, 4 synthetic
    // `}`, 5 eof — upstream tokenizeStringInterpolation scans the literal as
    // STRING segments around the expression, and appendEofToken's
    // unmatchedBeginGroup parks the synthetic `}` at eof after the
    // unterminated tail segment (upstream Dart SDK source,
    // `pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart`).
    let t = &r.tokens;
    assert_eq!(t.matching_end(1), Some(4)); // `${` → the eof-parked synthetic
    assert!(t.get(3).unwrap().is_synthetic());
    assert_eq!(t.get(3).unwrap().byte_range(), 5..5);
    assert!(t.get(4).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(1, 4)]);
}

#[test]
fn stray_close_brace_no_scanner_diag() {
    // Extra/mismatched close tokens: the scanner does not report (parser's job, official appendEndGroupInternal).
    let r = scan_src("foo())");
    assert!(r.diagnostics.is_empty());
    let r = scan_src("a } b");
    assert!(r.diagnostics.is_empty());
    assert_kinds(&r, &[K::Identifier, K::CloseCurly, K::Identifier]);
}

#[test]
fn deeper_match_reports_intermediate_at_closer() {
    // `{[}`: `}` matches `{` deep in the stack; the `[` above it is closed by
    // a synthetic `]` at the mismatching closer and recorded unmatched
    // (official insertSyntheticClosers; the parser reports the row post-parse
    // at `begin.endGroup` — oracle `dart analyze --format=json` on `{[}\n`:
    // expected_token "Expected to find ']'." @2 len1 — the `}` position,
    // not eof-1).
    let r = scan_src("{[}");
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 {, 1 [, 2 synthetic ], 3 }: `[` → synthetic, `{` → the real `}`.
    assert_eq!(t.matching_end(0), Some(3));
    assert_eq!(t.matching_end(1), Some(2));
    assert!(t.get(2).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(1, 2)]);
}

#[test]
fn unmatched_paren_reported_at_mismatching_close() {
    // `main { f(} }`: the `(` above the deep `{` is closed by a synthetic
    // `)` at the mismatching closer and recorded unmatched (official
    // insertSyntheticClosers; the parser reports the row post-parse at
    // `begin.endGroup`'s offset — translate_error_token.dart UnmatchedToken —
    // the synthetic parks at the first `}`); the `{` itself closes with that
    // `}`; the stray second `}` is silent.
    let src = "main { f(} }";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 main, 1 `{`@5, 2 f, 3 `(`@8, 4 synthetic `)`@9, 5 `}`@9, 6 `}`@11, 7 eof.
    assert_eq!(t.matching_end(1), Some(5)); // `{` closed by the first `}`
    assert_eq!(t.matching_end(3), Some(4)); // `(` → synthetic at the closer
    assert!(t.get(4).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(3, 4)]);
}

#[test]
fn curly_without_match_closes_head_at_closer() {
    // `f(}`: a `}` with no `{` anywhere still closes off the stack head at
    // the closer: the `(` is recorded unmatched at the `}` (oracle json on
    // `f(}\n` and `f(}\nhello\n` both show expected_token ')' @2 len1 — the
    // `}`, reported by the parser post-parse — and **nothing** at eof).
    let r = scan_src("f(}");
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 f, 1 (, 2 synthetic ), 3 }
    assert_eq!(t.matching_end(1), Some(2));
    assert!(t.get(2).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(1, 2)]);
    // Content after the mismatch does not move or duplicate the pair.
    let r = scan_src("f(}\nhello\n");
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    assert_eq!(r.tokens.unmatched_groups(), &[(1, 2)]);
}

#[test]
fn paren_bracket_without_match_silent() {
    // `)`/`]` with no matching opener anywhere are silent (official
    // appendEndGroupInternal leaves the stack alone): oracle json on
    // `{a);}\n` and `main() { a]; }\n` → no scanner expected_token at all;
    // the later `}` closes `{` normally.
    let r = scan_src("{a);}");
    assert!(r.diagnostics.is_empty());
    assert_eq!(r.tokens.matching_end(0), Some(4));
    let r = scan_src("main() { a]; }");
    assert!(r.diagnostics.is_empty());
}

#[test]
fn deep_match_drop_official_example() {
    // `[(])]` (official discardBeginGroupUntil comment, abstract_scanner.dart
    // :775-776): at the `]` the `[` matches one begin down, so both postures
    // run to EOF on cloned state. Insert = 1 synthetic `)` + no-match `)` +
    // no-match `]` + empty EOF stack = 3 recoveries; drop = untouched stack,
    // both later closers pair head-first, +1 for the ignored closer = 1.
    // 1 < 3 strictly → drop: the closer stands alone, the begins pair with
    // the LATER real closers, zero scanner diagnostics (upstream
    // bracket_mismatch_00.dart.expect reports only the parser-level pair).
    let r = scan_src("[(])]");
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 [, 1 (, 2 ], 3 ), 4 ] — every closer a real token.
    assert!(
        !t.as_slice()
            .iter()
            .any(super::super::token::Token::is_synthetic)
    );
    assert_eq!(t.matching_end(0), Some(4)); // `[` closed by the final `]`
    assert_eq!(t.matching_end(1), Some(3)); // `(` closed by the first `)`
}

#[test]
fn deep_match_drop_corpus_bracket_mismatch_00() {
    // The `bracket_mismatch_00.dart` bad half: the `]` mismatches the `(` one
    // begin down. Insert = synthetic `)` + no-match `)` + no-match `]` = 3
    // recoveries; drop = all three closers pair with their real begins, +1 =
    // 1 → drop (upstream .dart.expect: no scanner problem, the paren closes
    // at the real `)`).
    let src = "void bad() {\n  [\n    (\n    ])\n  ];\n}\n";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 void, 1 bad, 2 (, 3 ), 4 {, 5 [, 6 (, 7 ], 8 ), 9 ], 10 ;, 11 }.
    assert!(
        !t.as_slice()
            .iter()
            .any(super::super::token::Token::is_synthetic)
    );
    assert_eq!(t.matching_end(4), Some(11)); // `{` — the final `}`
    assert_eq!(t.matching_end(5), Some(9)); // `[` — the `]` of `];`
    assert_eq!(t.matching_end(6), Some(8)); // `(` — the `)` right after the junk `]`
    assert_eq!(t.get(7).unwrap().kind, K::CloseSquare);
    assert_eq!(t.get(7).unwrap().byte_offset, 27); // the junk `]`, a plain token
}

#[test]
fn deep_match_drop_corpus_bracket_mismatch_04() {
    // `foo(() {bar());});` (the bracket_mismatch_04 body): the stray `)`)
    // after `bar()` mismatches the `(` of `foo(` one begin down (the block
    // `{` sits above it). Insert = synthetic `}` + no-match `)` + no-match
    // `}` = 3 recoveries; drop = `}`/`)`/`}` all pair head-first, +1 = 1 →
    // drop, zero scanner diagnostics (upstream bracket_mismatch_04.dart.expect
    // reports only parser-level rows).
    let r = scan_src("void foo() {\n  foo(() {bar());});\n}\n");
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 void, 1 foo, 2 (, 3 ), 4 {, 5 foo, 6 (, 7 (, 8 ), 9 {, 10 bar,
    // 11 (, 12 ), 13 ), 14 ;, 15 }, 16 ), 17 ;, 18 }.
    assert!(
        !t.as_slice()
            .iter()
            .any(super::super::token::Token::is_synthetic)
    );
    assert_eq!(t.matching_end(4), Some(18)); // `void foo() {` body
    assert_eq!(t.matching_end(6), Some(16)); // `foo(` — the `)` after the block `}`
    assert_eq!(t.matching_end(9), Some(15)); // `{` — the block's own `}`
}

#[test]
fn deep_match_insert_stays_when_not_strictly_cheaper() {
    // Insert-stays shapes (option 2 not strictly cheaper → the default
    // insert keeps the pre-port shape).
    //
    // `{[1};`: insert = 1 synthetic + 0 EOF stack = 1; drop = 0 + 2 unclosed
    // + 1 = 3 → insert. The `[` is closed by a synthetic `]` at the `}` and
    // recorded unmatched (the parser reports the row there post-parse —
    // existing `{[}` pin: errors::deeper_match…; `main { f(} }`
    // tie→insert pin: errors::unmatched_paren…).
    let src = "{[1};";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 {, 1 [, 2 1, 3 synthetic ], 4 }.
    assert_eq!(t.matching_end(0), Some(4));
    assert_eq!(t.matching_end(1), Some(3));
    assert!(t.get(3).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(1, 3)]);

    // EOF-truncated `void f() { [ (])`: insert = 1 synthetic + no-match `)`
    // + 1 unclosed `{` at eof = 3; drop = 0 + 2 unclosed + 1 = 3 — a tie
    // inserts (official default). The synthetic `)` lands at the `]`; the
    // `[` is the insert match and closes at the REAL `]`, so only the `{`
    // remains for eof (appendEofToken: one pair per still-unclosed begin).
    let src = "void f() { [ (])";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 0 void, 1 f, 2 (, 3 ), 4 {, 5 [, 6 (, 7 synthetic ), 8 ], 9 ), 10 eof }.
    assert_eq!(t.matching_end(6), Some(7));
    assert!(t.get(7).unwrap().is_synthetic());
    assert_eq!(t.matching_end(5), Some(8)); // `[` — the real `]` (insert match)
    let eof_n = t.len();
    assert_eq!(t.matching_end(4), Some(eof_n - 2)); // `{` — the eof synthetic
    // Pairs: the `(` at the `]`-parked synthetic, the `{` at eof.
    assert_eq!(t.unmatched_groups(), &[(6, 7), (4, (eof_n - 2) as u32)]);
}

#[test]
fn gt_closes_lt_silently() {
    // Generic closing does not error; `>>` records endGroup only for the outermost `<` (official appendGtGt).
    let r = scan_src("List<List<int>> x");
    assert!(r.diagnostics.is_empty());
    let t = &r.tokens;
    // 0 List, 1 <, 2 List, 3 <, 4 int, 5 >>, 6 x
    assert_eq!(t.matching_end(1), Some(5));
    assert_eq!(t.matching_end(3), None); // inner one unrecorded (official semantics)
    assert_eq!(t.matching_end(1), Some(5));
}

#[test]
fn single_gt_closes_inner_lt() {
    // A single `>` closes one level: the `>` of `Map<String, int>` is recorded for the `<`.
    let r = scan_src("Map<String, int> m");
    assert!(r.diagnostics.is_empty());
    assert_eq!(r.tokens.matching_end(1), Some(5));
}

#[test]
fn discard_open_lt_on_semicolon_brace() {
    // `;` / `{` / `[` / `${` / `=` silently discard an unclosed `<` (official discardOpenLt).
    let r = scan_src("if (a < b) {}");
    assert!(r.diagnostics.is_empty());
    // By the time `{` appears, the `<` has been silently popped → the `{` begin closes normally.
    let t = &r.tokens;
    let open_curly = t
        .as_slice()
        .iter()
        .position(|x| x.kind == K::OpenCurly)
        .unwrap();
    assert!(t.matching_end(open_curly).is_some());
}

#[test]
fn dart_version_comment_in_window() {
    let r = scan_src("// @dart = 2.19\nvar x;");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(2, 19))
    );
}

#[test]
fn dart_version_comment_formats() {
    // No-space form.
    let r = scan_src("//@dart=2.12\nx");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(2, 12))
    );
    // Multi-space form.
    let r = scan_src("//   @dart   =   3.15  \nx");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(3, 15))
    );
    // Comments following other comments are still inside the window.
    let r = scan_src("// lead\n// @dart = 2.19\n// trail\nx");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(2, 19))
    );
    // A version comment after a script tag is still inside the window.
    let r = scan_src("#!/usr/bin/env dart\n// @dart = 2.19\nx");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(2, 19))
    );
    // Multiple matches: the last one wins (oracle `_languageVersionChanged`
    // reassigns the override on every match, analyzer-8.4.1
    // scanner.dart:195-222).
    let r = scan_src("// @dart = 2.1\n// @dart = 2.19\nx");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(2, 19))
    );
}

#[test]
fn dart_version_comment_invalid_forms() {
    // `///` is not a version comment.
    let r = scan_src("/// @dart = 2.19\nx");
    assert_eq!(r.language_version_override, None);
    // Tab indentation does not match the syntax (the official scanner only eats spaces).
    let r = scan_src("//\t@dart = 2.19\nx");
    assert_eq!(r.language_version_override, None);
    // Trailing garbage.
    let r = scan_src("// @dart = 2.19 nope\nx");
    assert_eq!(r.language_version_override, None);
    // Missing minor.
    let r = scan_src("// @dart = 2\nx");
    assert_eq!(r.language_version_override, None);
    // Outside the window (a real token already emitted).
    let r = scan_src("var x; // @dart = 2.19");
    assert_eq!(r.language_version_override, None);
    // Block comments do not trigger recognition.
    let r = scan_src("/* @dart = 2.19 */\nx");
    assert_eq!(r.language_version_override, None);
}

#[test]
fn eof_after_version_comment_only() {
    let r = scan_src("// @dart = 2.19");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(2, 19))
    );
    assert_eq!(r.tokens.len(), 1);
}

#[test]
fn dart_version_comment_span() {
    fn span_of(src: &str) -> Option<(u32, u32)> {
        let r = scan_src(src);
        r.language_version_override_span
            .map(|s| (s.offset, s.length))
    }

    // Whole comment lexeme: `//` start, 15 units (oracle Dart 3.13.1
    // `dart analyze --format=machine`: `1|1|15`; official test
    // invalid_language_override_greater_test.dart: `[column 1][length 15]`).
    assert_eq!(span_of("// @dart = 2.19\nvar x;"), Some((0, 15)));
    // Leading indentation shifts the start (oracle: `1|3|15`).
    assert_eq!(span_of("  // @dart = 2.19\nx"), Some((2, 15)));
    // Comments after other comments stay in the window (oracle: `2|1|15`).
    assert_eq!(span_of("// lead\n// @dart = 2.19\nx"), Some((8, 15)));
    // Trailing spaces belong to the comment token (oracle: length 18 for
    // `// @dart = 99.9   `).
    assert_eq!(span_of("//@dart=3.15  \nx"), Some((0, 14)));
    // CRLF: the CR is the terminator, not comment content (oracle: `1|1|15`).
    assert_eq!(span_of("// @dart = 2.19\r\nx"), Some((0, 15)));
    // After a script tag (window stays open).
    assert_eq!(
        span_of("#!/usr/bin/env dart\n// @dart = 2.19\nx"),
        Some((20, 15))
    );
    // UTF-16 offsets: `é` on the leading line counts 1 unit, 3 bytes.
    assert_eq!(span_of("// é\n// @dart = 2.19\nx"), Some((5, 15)));
    // Last match wins; the span equals the winning (last) comment's lexeme.
    assert_eq!(
        span_of("// @dart = 2.1\n// @dart = 2.19\nx"),
        Some((15, 15))
    );
    // Two different versions: the last one wins even though the first was
    // also valid (second below latest → no above-latest warning in the
    // oracle); the winning override is the last comment's version, and the
    // span follows it.
    let r = scan_src("// @dart = 2.19\n// @dart = 2.9\nx");
    assert_eq!(
        r.language_version_override,
        Some(dartq_language::LanguageVersion::new(2, 9))
    );
    assert_eq!(
        span_of("// @dart = 2.19\n// @dart = 2.9\nx"),
        Some((16, 14))
    );
    // No override → no span.
    let r = scan_src("var x; // @dart = 2.19");
    assert_eq!(r.language_version_override, None);
    assert_eq!(r.language_version_override_span, None);
}

#[test]
fn del_merges_into_identifier() {
    // DEL (0x7F) takes the official default arm (error_token.dart:17-47 →
    // NonAsciiIdentifierToken): merges with the adjacent identifier, carrying
    // the registry illegal_character template like every other class.
    let src = "ab\u{7F}cd";
    let r = scan_src(src);
    assert_kinds(&r, &[K::Identifier]);
    assert_codes(&r, &["illegal_character"]);
    assert_eq!(r.tokens.first().byte_range(), 0..src.len());
    // DEL adjacent to a preceding identifier folds into it.
    let r = scan_src("ab\u{7F}");
    assert_kinds(&r, &[K::Identifier]);
    assert_eq!(r.tokens.first().byte_range(), 0..3);
}

#[test]
fn replacement_char_is_skipped_silently() {
    // U+FFFD: zero diagnostics. The official scanner has an EncodingErrorToken
    // path, but the pinned oracle emits nothing for either reachable U+FFFD
    // class — a raw undecodable byte (read-layer tolerance) and a literal
    // U+FFFD in valid UTF-8 — so the measured-cull doctrine governs
    //. The character is consumed,
    // never merged into an identifier.
    let r = scan_src("a\u{FFFD}b");
    assert_kinds(&r, &[K::Identifier, K::Identifier]);
    assert!(r.diagnostics.is_empty());
    // Consecutive U+FFFD: still silent, no token merging.
    let r = scan_src("\u{FFFD}\u{FFFD}x");
    assert_kinds(&r, &[K::Identifier]);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn unterminated_comment_multibyte_last_char() {
    // F6: the diagnostic pins to the last character (analyzer endOffset-1) —
    // the UTF-16 range stays a single unit even when that character is
    // multi-byte (`a /* 你好` → the range covers 好, utf16 [6,7)).
    let src = "a /* 你好";
    let r = scan_src(src);
    assert_codes(&r, &["unterminated_multi_line_comment"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 6);
    assert_eq!(d.range.length, 1);
}

#[test]
fn unterminated_string_astral_last_char() {
    // The analyzer pin is `endOffset - 1` in UTF-16 units — for an astral
    // final character that is its LOW surrogate, a single unit (oracle
    // `dart analyze` on `'abc 😀`: offset eof-1, length 1), not the
    // character start with length 2.
    let src = "'abc 😀"; // utf16: ' 0, a 1, b 2, c 3, sp 4, emoji 5-6, eof 7
    let r = scan_src(src);
    assert_codes(&r, &["unterminated_string_literal"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 6);
    assert_eq!(d.range.length, 1);
    // Newline-terminated form: one unit before the newline (same rule).
    let r = scan_src("'abc 😀\n");
    assert_codes(&r, &["unterminated_string_literal"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 6);
    assert_eq!(d.range.length, 1);
    // CRLF: the CR is the stop position; one unit before it.
    let r = scan_src("'abc 😀\r\n");
    assert_codes(&r, &["unterminated_string_literal"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 6);
    assert_eq!(d.range.length, 1);
    // Raw form: same pin rule (oracle `r'abc 😀`: eof-1).
    let r = scan_src("r'abc 😀"); // utf16: r 0, ' 1, a 2..sp 5, emoji 6-7, eof 8
    assert_codes(&r, &["unterminated_string_literal"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 7);
    assert_eq!(d.range.length, 1);
    // Multiline form with inner line break: pin = eof-1 (oracle `'''abc\r\n😀`).
    let r = scan_src("'''abc\r\n😀"); // utf16: ''' 0-2, abc 3-5, \r 6, \n 7, emoji 8-9, eof 10
    assert_codes(&r, &["unterminated_string_literal"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 9);
    assert_eq!(d.range.length, 1);
}

#[test]
fn unterminated_comment_astral_last_char() {
    // Same rule as strings: the pin is the last UTF-16 unit — the emoji's
    // low surrogate — not the character start (oracle `a /* 😀`: eof-1).
    let src = "a /* 😀"; // utf16: a 0, sp 1, / 2, * 3, sp 4, emoji 5-6, eof 7
    let r = scan_src(src);
    assert_codes(&r, &["unterminated_multi_line_comment"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 6);
    assert_eq!(d.range.length, 1);
}

#[test]
fn eof_expected_token_astral_last_char() {
    // An unclosed `(` behind a trailing astral character: the scanner's own
    // rows are the two surrogate illegal_character pins (high then low, one
    // unit each); the `expected_token` row rides the parser's post-parse
    // report (official unmatchedBeginGroup records the pair, the analyzer
    // translates it post-parse) — pinned at the eof position regardless of
    // the final character's width (oracle json on `f(😀`: expected_token
    // ')' at the utf16 eof offset — one unit past the emoji's low
    // surrogate).
    let r = scan_src("f(😀"); // utf16: f 0, ( 1, emoji 2-3, eof 4
    assert_codes(&r, &["illegal_character", "illegal_character"]);
    let d0 = &r.diagnostics[0];
    assert_eq!(d0.range.offset, 2); // high surrogate
    assert_eq!(d0.range.length, 1);
    let d1 = &r.diagnostics[1];
    assert_eq!(d1.range.offset, 3); // low surrogate
    assert_eq!(d1.range.length, 1);
    // The `(` closes with the eof-synthetic closer (second to last, just
    // before Eof) and the pair feeds the parser's expected_token row.
    let t = &r.tokens;
    let n = t.as_slice().len();
    assert_eq!(t.matching_end(1), Some(n - 2));
    assert!(t.get(n - 2).unwrap().is_synthetic());
    assert_eq!(t.unmatched_groups(), &[(1, (n - 2) as u32)]);
}

#[test]
fn dollar_missing_identifier_astral_char() {
    // `$` + astral char: missing_identifier pins the character itself — a
    // single UTF-16 unit at the character start (oracle `'$😀`: high
    // surrogate, length 1), not the whole surrogate pair.
    let src = "'$😀"; // utf16: ' 0, $ 1, emoji 2-3, eof 4
    let r = scan_src(src);
    assert_codes(&r, &["missing_identifier", "unterminated_string_literal"]);
    let d = &r.diagnostics[0];
    assert_eq!(d.range.offset, 2);
    assert_eq!(d.range.length, 1);
    // The string's own unterminated pin is the low surrogate (eof-1).
    let d = &r.diagnostics[1];
    assert_eq!(d.range.offset, 3);
    assert_eq!(d.range.length, 1);
}

#[test]
fn empty_block_comment_is_not_unterminated() {
    // `/**/` scans as a plain comment (regression: the block-comment entry
    // used to re-bump and swallow one content character, mis-scanning `/**/`).
    let r = scan_src("/**/x");
    assert_kinds(&r, &[K::Identifier]);
    assert!(r.diagnostics.is_empty());
    let r = scan_src("/***/x");
    assert_kinds(&r, &[K::Identifier]);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn this_discards_open_lt() {
    // `this` discards an open `<` group (official appendKeywordToken: type
    // parameters cannot contain 'this') → the later `>` closes nothing.
    let r = scan_src("x < y ? this : z > w");
    assert!(r.diagnostics.is_empty());
    let t = &r.tokens;
    let lt = t
        .as_slice()
        .iter()
        .position(|tok| tok.kind == K::Lt)
        .unwrap();
    assert_eq!(t.matching_end(lt), None);
    // Same inside interpolation: `'$this'` (keyword arm of the SII path).
    let r = scan_src("'${a < b ? this : c}'");
    assert!(r.diagnostics.is_empty());
    let t = &r.tokens;
    let lt = t
        .as_slice()
        .iter()
        .position(|tok| tok.kind == K::Lt)
        .unwrap();
    assert_eq!(t.matching_end(lt), None);
}

#[test]
fn diagnostics_cap_bounds_adversarial_input() {
    // A hostile source with more illegal characters than the recording cap
    // must not stream unbounded diagnostics (each carries a fresh format!
    // message): recording stops at the cap, parser parity (dartq-parser
    // MAX_DIAGNOSTICS), while scanning still runs to EOF — the token stream
    // is unchanged and the Eof sentinel is present.
    let src = format!("var x = 1;{}", "\u{00A0}".repeat(10_500));
    let r = scan_src(&src);
    assert_eq!(r.diagnostics.len(), 10_000);
    assert!(codes(&r).iter().all(|c| *c == "illegal_character"));
    assert_kinds(
        &r,
        &[
            K::Keyword(Keyword::Var),
            K::Identifier,
            K::Eq,
            K::Int,
            K::Semicolon,
        ],
    );
    let eof = r.tokens.get(r.tokens.len() - 1).unwrap();
    assert_eq!(eof.kind, K::Eof);
}
