//! The sii-probes corpus wave (ledger GAP-SII, ticket #1689): one test per
//! frozen regression case in `tests/corpus/regressions/cases/`, pinning the
//! scanner-face rows of the official recovery shapes against upstream Dart
//! SDK source, `pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart`
//! (string/interpolation paths) and `pkg/analyzer/lib/src/dart/scanner/
//! translate_error_token.dart` (the diagnostic translation: unterminated
//! strings report `endOffset - 1, length 1`; scanner `UnmatchedToken`s are
//! carried as `(begin, synthetic close)` pairs the parser reports post-parse).
//!
//! Per case the tests pin: the `unterminated_string_literal` span, the
//! synthetic STRING segment boundaries (official `SyntheticStringToken`:
//! content from the current segment start, empty after an EOF cascade), the
//! unmatched `(begin offset, closer offset)` pairs in scan-close order, and
//! where load-bearing the kind sequence. Parser-face rows of the same cases
//! (`expected_token` / `unused_element`) are the dartq-parser's post-parse
//! report over these same shapes — out of scanner scope.

use super::{assert_kinds, scan_src};
use crate::ScanResult;
use crate::TokenKind as K;

/// Diagnostic rows as `(code, utf16 offset, length)`.
fn rows(r: &ScanResult) -> Vec<(&'static str, u32, u32)> {
    r.diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// Unmatched pairs as `(begin byte offset, synthetic closer byte offset)`;
/// asserts every closer is synthetic.
fn unmatched_shapes(r: &ScanResult) -> Vec<(u32, u32)> {
    r.tokens
        .unmatched_groups()
        .iter()
        .map(|&(b, c)| {
            let begin = r.tokens.get(b as usize).unwrap();
            let close = r.tokens.get(c as usize).unwrap();
            assert!(
                close.is_synthetic(),
                "unmatched closer must be synthetic: {b}->{c}"
            );
            (begin.byte_offset, close.byte_offset)
        })
        .collect()
}

/// `0a523463` — `f('${{`: only the string reports unterminated (pin on the
/// line break, oracle 1:7); the `${` consumes both characters, so the second
/// `{` stays an ordinary code-mode begin group and the three open groups
/// close with eof-synthetic closers in upstream
/// discardInterpolation/appendEofToken order (`{`@5, `${`@3, `(`@1).
#[test]
fn sii_0a523463_two_unclosed_curly() {
    let r = scan_src("f('${{\n");
    assert_eq!(
        rows(&r),
        &[("unterminated_string_literal", 6, 1)],
        "{:?}",
        r.diagnostics
    );
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::OpenCurly,  // `{`@5, below the `${`
            K::String,     // eof-cascade unterminated segment
            K::CloseCurly, // synthetic for `{`@5
            K::CloseCurly, // synthetic for `${`@3
            K::CloseParen, // synthetic for `(`@1
        ],
    );
    // Prefix segment ends before `$`; the unterminated segment is empty
    // (upstream resumes the suspended body with start = scanOffset = eof).
    assert_eq!(r.tokens.get(2).unwrap().byte_range(), 2..3);
    assert_eq!(r.tokens.get(5).unwrap().byte_range(), 7..7);
    assert_eq!(unmatched_shapes(&r), &[(5, 7), (3, 7), (1, 7)]);
}

/// `0e2b6063` — `'''${`: multiline body suspended by the `${`; the EOF
/// cascade resumes it at the eof position (oracle 1:6 = eof - 1) and the
/// open `${` closes with a synthetic `}`.
#[test]
fn sii_0e2b6063_bare_triple() {
    let r = scan_src("'''${\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 5, 1)]);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationExpression,
            K::String,
            K::CloseCurly,
        ],
    );
    assert_eq!(r.tokens.get(0).unwrap().byte_range(), 0..3);
    assert_eq!(r.tokens.get(2).unwrap().byte_range(), 6..6);
    assert_eq!(unmatched_shapes(&r), &[(3, 6)]);
}

/// `1020e348` — `var s = '${})`: the empty interpolation closes at `}`, the
/// body resumes and takes `)` as content; the break lands on the line feed
/// (oracle 1:13 = endOffset - 1) and the final segment carries it (upstream
/// `SyntheticStringToken` [12, 13)).
#[test]
fn sii_1020e348_empty_interp_no_call() {
    let r = scan_src("var s = '${})\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 12, 1)]);
    assert_kinds(
        &r,
        &[
            K::Keyword(crate::Keyword::Var),
            K::Identifier,
            K::Eq,
            K::String,
            K::StringInterpolationExpression,
            K::CloseCurly,
            K::String,
        ],
    );
    assert_eq!(r.tokens.get(3).unwrap().byte_range(), 8..9);
    assert_eq!(r.tokens.get(6).unwrap().byte_range(), 12..13);
    // The `${` matched the real `}`; the `(`-less `)` never scans as a token.
    assert!(r.tokens.unmatched_groups().is_empty());
}

/// `27b489d9` — `'${`: EOF right after the `${` (oracle 1:4 = the break);
/// the segment is empty and the `${` pairs with its eof-synthetic `}`.
#[test]
fn sii_27b489d9_bare_interp() {
    let r = scan_src("'${\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 3, 1)]);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationExpression,
            K::String,
            K::CloseCurly,
        ],
    );
    assert_eq!(r.tokens.get(2).unwrap().byte_range(), 4..4);
    assert_eq!(unmatched_shapes(&r), &[(1, 4)]);
}

/// `2cac8d56` — `f('${x)`: the `)` deep-matches the enclosing `(` below the
/// open `${`; the dual-option scoring inserts a synthetic `}` at the `)`
/// (official `discardBeginGroupUntil` option run: insert 2 = base 1 + the
/// unmatched `${` vs drop 3 = base 1 + the open paren + the ignore penalty)
/// and records the `${` as unmatched at offset 6 (the parser's
/// `expected_token '}'` row, oracle 1:7). The body never resumes, so the
/// eof-cascade segment is empty.
#[test]
fn sii_2cac8d56_deep_close_after_content() {
    let r = scan_src("f('${x)\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 7, 1)]);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::CloseCurly, // synthetic for `${`@3, inserted at the `)`
            K::CloseParen, // real, closes `(`@1
            K::String,
        ],
    );
    assert_eq!(r.tokens.get(5).unwrap().byte_offset, 6);
    assert_eq!(r.tokens.get(7).unwrap().byte_range(), 8..8);
    assert_eq!(r.tokens.matching_end(1), Some(6));
    assert_eq!(unmatched_shapes(&r), &[(3, 6)]);
}

/// `38a8f893` — `f('${x})`: the `}` closes the `${` for real ($STX) — no
/// deep-match decision exists at the `)`, which after the close is string
/// content, not a code-mode closer — and the body resumes at the `)`,
/// breaking on the line feed: the unterminated pin lands on the last content
/// unit (endOffset - 1 = 7; live probe UNTERMINATED_STRING_LITERAL 1|8|1,
/// captured corpus rows agree — no `expected_token '}'` row). The resumed
/// segment carries the `)` (official `SyntheticStringToken` over [7, 8)),
/// and the still-open `(` parks at eof as the parser-side
/// `expected_token ')'` pair. Version attribution: the vendored Dart SDK
/// reference (3.15 main) `discardBeginGroupUntil` scores the
/// paren-below-open-interpolation posture a tie whose default is INSERT
/// (synthetic `}` at the `)`, pin at eof - 1); the parity oracle of record
/// (3.13.1 corpus + live probe) drops. The pin follows the oracle of record;
/// the ledger carries the attribution.
#[test]
fn sii_38a8f893_close_then_paren() {
    let r = scan_src("f('${x})\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 7, 1)]);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::CloseCurly, // real, closes the `${` ($STX)
            K::String,     // resumed body: the `)` up to the break
            K::CloseParen, // eof-synthetic for the still-open `(`
        ],
    );
    let t = &r.tokens;
    assert!(!t.get(5).unwrap().is_synthetic());
    assert_eq!(t.get(5).unwrap().byte_offset, 6);
    assert!(t.get(6).unwrap().is_synthetic());
    assert_eq!(t.get(6).unwrap().byte_range(), 7..8);
    assert_eq!(t.matching_end(3), Some(5)); // `${` closed by the real `}`
    assert_eq!(t.matching_end(1), Some(7)); // `(` parked at eof
    assert_eq!(unmatched_shapes(&r), &[(1, 9)]);
}

/// `42295fda` — `'$\n`: the bare `$` takes the identifier form (oracle
/// missing_identifier 1:3 = one unit past the `$`), then the line break
/// terminates the body (oracle unterminated 1:2). The final segment is the
/// empty [2, 2): upstream `unterminatedString` spans `[start, stringOffset)`
/// (upstream Dart SDK source,
/// `pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart`) and
/// stringOffset sits at the terminator — the oracle's own `unterminated 1:2`
/// row is `endOffset 2 - 1`, proving the break is not string content.
#[test]
fn sii_42295fda_bare_dollar() {
    let r = scan_src("'$\n");
    assert_eq!(
        rows(&r),
        &[
            ("missing_identifier", 2, 1),
            ("unterminated_string_literal", 1, 1),
        ]
    );
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationIdentifier,
            K::Identifier,
            K::String,
        ],
    );
    let ident = r.tokens.get(2).unwrap();
    assert!(ident.is_synthetic());
    assert_eq!(ident.byte_range(), 2..2);
    assert_eq!(r.diagnostics[0].message, "Expected an identifier.");
    assert_eq!(r.tokens.get(3).unwrap().byte_range(), 2..2);
    assert_eq!(r.diagnostics[1].message, "Unterminated string literal.");
    // `$id` interpolation is not a group: nothing unmatched.
    assert!(r.tokens.unmatched_groups().is_empty());
}

/// `$` directly at EOF (the 42295fda family without the line break): the
/// missing_identifier pin stays at the position after the `$` — the eof
/// position — and keeps `length: 1`: the official translation reports one
/// virtual UTF-16 unit past the text (upstream Dart SDK source,
/// `pkg/analyzer/lib/src/dart/scanner/translate_error_token.dart` — the
/// synthetic identifier follows the error in-stream, `_isAtEnd` never backs
/// the pin off, and a missing token at the eof boundary still carries
/// `length: 1`; live probe MISSING_IDENTIFIER 1|3|1 on `'$`). The conversion
/// walk admits a target exactly one past the source end for this shape.
#[test]
fn sii_dollar_eof_missing_identifier_pin() {
    let r = scan_src("'$");
    assert_eq!(
        rows(&r),
        &[
            ("missing_identifier", 2, 1),
            ("unterminated_string_literal", 1, 1),
        ]
    );
    let ident = r.tokens.get(2).unwrap();
    assert!(ident.is_synthetic());
    assert_eq!(ident.byte_range(), 2..2);
    // The unterminated segment starts after the `$id` (eof, empty).
    assert_eq!(r.tokens.get(3).unwrap().byte_range(), 2..2);
}

/// `44f48958` — `f('${`: the string and both open groups recover at eof
/// (oracle unterminated 1:6; the `${` closer feeds the parser's
/// `expected_token '}'` 2:1 row, the `(` closer is swallowed by the parser's
/// argument-list recovery).
#[test]
fn sii_44f48958_deep_close_bare() {
    let r = scan_src("f('${\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 5, 1)]);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::String,
            K::CloseCurly,
            K::CloseParen,
        ],
    );
    assert_eq!(r.tokens.get(4).unwrap().byte_range(), 6..6);
    assert_eq!(unmatched_shapes(&r), &[(3, 6), (1, 6)]);
}

/// `5122b5cd` — `f('${f('${')`: three unterminated strings, innermost first
/// — the innermost body-local (pin on the `)`, oracle 1:12; its segment
/// carries `')`), then the two suspended bodies at the eof cascade (both
/// pins at eof - 1; their tokens translate to identical rows and collapse to
/// the single oracle 1:13 row — `RecordingDiagnosticListener`'s
/// `Set<Diagnostic>` semantics, mirrored by the finish-pass dedup). The four
/// open groups close innermost-first in upstream
/// discardInterpolation/appendEofToken order.
#[test]
fn sii_5122b5cd_nested_unclosed() {
    let r = scan_src("f('${f('${')\n");
    assert_eq!(
        rows(&r),
        &[
            ("unterminated_string_literal", 11, 1),
            ("unterminated_string_literal", 12, 1),
        ]
    );
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::String,     // innermost unterminated segment `')`
            K::String,     // eof cascade (middle body)
            K::String,     // eof cascade (outer body)
            K::CloseCurly, // synthetic for `${`@8
            K::CloseParen, // synthetic for `(`@6
            K::CloseCurly, // synthetic for `${`@3
            K::CloseParen, // synthetic for `(`@1
        ],
    );
    assert_eq!(r.tokens.get(8).unwrap().byte_range(), 10..12);
    assert_eq!(r.tokens.get(9).unwrap().byte_range(), 13..13);
    assert_eq!(unmatched_shapes(&r), &[(8, 13), (6, 13), (3, 13), (1, 13)]);
}

/// `64e8b192` — `g('${`: the 44f48958 shape under a different callee name.
#[test]
fn sii_64e8b192_empty_interp_noparen() {
    let r = scan_src("g('${\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 5, 1)]);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::String,
            K::CloseCurly,
            K::CloseParen,
        ],
    );
    assert_eq!(r.tokens.get(4).unwrap().byte_range(), 6..6);
    assert_eq!(unmatched_shapes(&r), &[(3, 6), (1, 6)]);
}

/// `91fc4830` — `var s = '${}abc`: the closed interpolation resumes the body,
/// which takes `abc` as content; the final segment spans it (upstream
/// [12, 15)) and the pin lands on the last `c` (oracle 1:15). The parser
/// anchors its `expected_token ';'` row (oracle 1:13, length 3) on this
/// segment boundary.
#[test]
fn sii_91fc4830_empty_interp_body_after() {
    let r = scan_src("var s = '${}abc\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 14, 1)]);
    assert_kinds(
        &r,
        &[
            K::Keyword(crate::Keyword::Var),
            K::Identifier,
            K::Eq,
            K::String,
            K::StringInterpolationExpression,
            K::CloseCurly,
            K::String,
        ],
    );
    assert_eq!(r.tokens.get(6).unwrap().byte_range(), 12..15);
    assert!(r.tokens.unmatched_groups().is_empty());
}

/// `9bb24279` — `'${}`: the interpolation closes, the body resumes at the
/// suffix position and immediately hits the line feed — an empty final
/// segment (upstream start = scanOffset = 4) with the pin on the break
/// (oracle 1:4).
#[test]
fn sii_9bb24279_empty_interp_bare() {
    let r = scan_src("'${}\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 3, 1)]);
    assert_kinds(
        &r,
        &[
            K::String,
            K::StringInterpolationExpression,
            K::CloseCurly,
            K::String,
        ],
    );
    assert_eq!(r.tokens.get(3).unwrap().byte_range(), 4..4);
    assert_eq!(r.tokens.matching_end(1), Some(2));
    assert!(r.tokens.unmatched_groups().is_empty());
}

/// `cdb0d429` — `var s = '${x}abc`: the interpolation expression carries the
/// identifier, the body resumes after the `}` and `abc` becomes the final
/// segment (upstream [13, 16)); pin on the last `c` (oracle 1:16).
#[test]
fn sii_cdb0d429_nonempty_interp_body_after() {
    let r = scan_src("var s = '${x}abc\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 15, 1)]);
    assert_kinds(
        &r,
        &[
            K::Keyword(crate::Keyword::Var),
            K::Identifier,
            K::Eq,
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::CloseCurly,
            K::String,
        ],
    );
    assert_eq!(r.tokens.get(7).unwrap().byte_range(), 13..16);
    assert!(r.tokens.unmatched_groups().is_empty());
}

/// `d6ee108c` — `f('${)`: the `)` deep-matches the `(` below the open `${`
/// (insert wins 1 vs 3); the `${` is recorded unmatched at the `)` (the
/// parser's `expected_token '}'` row, oracle 1:6), the body never resumes
/// and the eof-cascade segment is empty (oracle unterminated 1:7).
#[test]
fn sii_d6ee108c_deep_close_paren() {
    let r = scan_src("f('${)\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 6, 1)]);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::CloseCurly, // synthetic for `${`@3, inserted at the `)`
            K::CloseParen, // real, closes `(`@1
            K::String,
        ],
    );
    assert_eq!(r.tokens.get(4).unwrap().byte_offset, 5);
    assert_eq!(r.tokens.get(6).unwrap().byte_range(), 7..7);
    assert_eq!(r.tokens.matching_end(1), Some(5));
    assert_eq!(unmatched_shapes(&r), &[(3, 5)]);
}

/// `dd217615` — `f('$${')`: the first `$` takes the identifier form (oracle
/// missing_identifier 1:5), the second opens a `${` whose nested string
/// breaks on the line feed (oracle 1:8; segment `')`), the suspended outer
/// body reports at the eof cascade (oracle 1:9), and the `(` plus the open
/// `${` close with eof-synthetic closers (the parser's `expected_token`
/// 2:1 row).
#[test]
fn sii_dd217615_double_dollar() {
    let r = scan_src("f('$${')\n");
    assert_eq!(
        rows(&r),
        &[
            ("missing_identifier", 4, 1),
            ("unterminated_string_literal", 7, 1),
            ("unterminated_string_literal", 8, 1),
        ]
    );
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,                        // `'`
            K::StringInterpolationIdentifier, // first `$`
            K::Identifier,                    // synthetic empty
            K::String,                        // empty segment before the second `$`
            K::StringInterpolationExpression, // `${`@4
            K::String,                        // nested unterminated segment `')`
            K::String,                        // eof cascade
            K::CloseCurly,                    // synthetic for `${`@4
            K::CloseParen,                    // synthetic for `(`@1
        ],
    );
    let ident = r.tokens.get(4).unwrap();
    assert!(ident.is_synthetic());
    assert_eq!(ident.byte_range(), 4..4);
    assert_eq!(r.diagnostics[0].message, "Expected an identifier.");
    assert_eq!(r.tokens.get(7).unwrap().byte_range(), 6..8);
    assert_eq!(r.tokens.get(8).unwrap().byte_range(), 9..9);
    assert_eq!(unmatched_shapes(&r), &[(4, 9), (1, 9)]);
}

/// `f23b17f6` — `f('${x`: the identifier is interpolation content, EOF ends
/// the expression; the eof-cascade segment is empty (oracle unterminated
/// 1:7 = eof - 1) and both groups close with synthetic closers (the parser's
/// `expected_token '}'` 2:1 row rides the `${` pair).
#[test]
fn sii_f23b17f6_deep_close_content() {
    let r = scan_src("f('${x\n");
    assert_eq!(rows(&r), &[("unterminated_string_literal", 6, 1)]);
    assert_kinds(
        &r,
        &[
            K::Identifier,
            K::OpenParen,
            K::String,
            K::StringInterpolationExpression,
            K::Identifier,
            K::String,
            K::CloseCurly,
            K::CloseParen,
        ],
    );
    assert_eq!(r.tokens.get(5).unwrap().byte_range(), 7..7);
    assert_eq!(unmatched_shapes(&r), &[(3, 7), (1, 7)]);
}
