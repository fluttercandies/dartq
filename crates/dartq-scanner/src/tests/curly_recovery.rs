//! Curly-bracket two-pass recovery (official `scan`/`scanString` re-scan,
//! scanner.dart:90-108): decision gates, both recovery sites, and the
//! oracle-pinned "user is typing" shapes from the `with_outline` corpus
//! family (`typing_01` / `typing_04` / `typing_string_interpolation_1`/2,
//! condensed to their recovery-bearing lines).

use super::{assert_codes, codes, scan_src, u32_of};
use crate::TokenKind as K;

/// `typing_01` shape: the `if {` the user just typed stays open, so the `}`
/// that should close `foo_method1` closes the `if` instead and `class Foo`'s
/// brace reaches EOF alone. The re-scan blames the `if {` (indent 4 vs the
/// closer's 2), pins a synthetic `}` at that closer, and the outer braces
/// close correctly (oracle: `expected_token` '}' @ the `}` col 3, nothing at
/// eof — exactly one diagnostic).
#[test]
fn two_pass_recovers_typed_open_if() {
    let src =
        "class Foo {\n  void foo_method1() {\n    if (a) { // missing }\n  }\n}\nclass Bar {}\n";
    let r = scan_src(src);
    // Pass 1 alone would park the missing `}` at eof; the re-scan moves the
    // pair to the misaligned closer (the `}` on line 4, byte 63) — the
    // parser reports the row post-parse at `begin.endGroup`.
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // 12 `{`=if (recovered), 13 synthetic `}`@63, 14 `}`@63 closes m1, 15 `}`@65 closes Foo.
    assert_eq!(t.get(12).unwrap().kind, K::OpenCurly);
    assert_eq!(t.get(12).unwrap().byte_offset, 46);
    assert_eq!(t.get(13).unwrap().kind, K::CloseCurly);
    assert!(t.get(13).unwrap().is_synthetic());
    assert_eq!(t.get(13).unwrap().byte_offset, 63);
    assert_eq!(t.matching_end(12), Some(13));
    assert_eq!(t.matching_end(7), Some(14));
    assert_eq!(t.matching_end(2), Some(15));
    // The blamed brace's pair is the stream's only unmatched group.
    assert_eq!(t.unmatched_groups(), &[(12, 13)]);
}

/// `typing_04` shape: an unclosed map literal `var x = {` — the `= {` branch
/// of the indent heuristic blames it, and the closer `}` moves onto the
/// enclosing `if` block (oracle shape: `expected_token` '}' @ the map's
/// closer, corrected outline above it).
#[test]
fn two_pass_recovers_unclosed_map_literal() {
    let src = "class Foo {\n  void m() {\n    var x = {1: 2 // missing }\n  }\n}\nclass Bar {}\n";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    let map_open = t
        .as_slice()
        .iter()
        .position(|tok| tok.kind == K::OpenCurly && tok.byte_offset == 37)
        .expect("map literal `{` at byte 37");
    let syn = t.matching_end(map_open).expect("map closed by recovery");
    assert!(t.get(syn).unwrap().is_synthetic());
    assert_eq!(t.get(syn).unwrap().byte_offset, 58);
    // The map literal's pair is the stream's only unmatched group; the row
    // reports post-parse at the synthetic (the line-4 `}`).
    assert_eq!(t.unmatched_groups(), &[(map_open as u32, syn as u32)]);
}

/// `typing_string_interpolation_1` shape: the `${` swallows a `"` inside the
/// expression, which (without recovery) leaks into code mode and cascades.
/// The re-scan discards the `${`: synthetic `}` at the `$`, and the trailing
/// `"` closes the literal again. Exactly one scanner diagnostic (oracle:
/// `expected_token` '}' @ the `$`, no `unterminated_string_literal` at all).
#[test]
fn two_pass_recovers_interpolation_start() {
    let src = "class B {\n  m1() {\n    print(\"${foo.length hello world\");\n  }\n}\n";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    // ... print ( String `${` SYN-} String(`foo.length hello world"`) ) } } }
    let sii = t
        .as_slice()
        .iter()
        .position(|tok| tok.kind == K::StringInterpolationExpression)
        .expect("SII token present");
    let syn = t.matching_end(sii).expect("SII closed by recovery");
    assert!(t.get(syn).unwrap().is_synthetic());
    assert_eq!(t.get(syn).unwrap().byte_offset, 30);
    // The string body resumes right after the `{` and the `"` at the end of
    // the line closes it: the suffix segment carries the words + the quote.
    let suffix = t.get(syn + 1).unwrap();
    assert_eq!(suffix.kind, K::String);
    assert_eq!(t.lexeme(src, syn + 1), "foo.length hello world\"");
    // The discarded `${` is the stream's only unmatched group; the `}` row
    // reports post-parse at the `$`.
    assert_eq!(t.unmatched_groups(), &[(sii as u32, syn as u32)]);
}

/// `typing_string_interpolation_2` shape: the `${` also spans a newline, so
/// after discarding it the resumed body unterminates at the line end and the
/// `");` line scans as its own unterminated string; the print `(` still
/// closes synthetically at the method's `}`. The scanner keeps only the two
/// unterminated rows; both `expected_token` rows (the `$` and the method `}`)
/// ride the pair list for the parser's post-parse report. Oracle pins
/// (machine format): `expected_token` '}' @ the `$`, unterminated @ the `.`,
/// unterminated @ the `;`, `expected_token` ')' @ the method `}`.
#[test]
fn two_pass_interpolation_across_newline() {
    let src = "class B {\n  m1() {\n    print(\"${foo.\n    length hello world\");\n  }\n}\n";
    let r = scan_src(src);
    assert_codes(
        &r,
        &["unterminated_string_literal", "unterminated_string_literal"],
    );
    let [d0, d1] = r.diagnostics.as_slice() else {
        panic!("two diagnostics expected: {:?}", codes(&r));
    };
    // Resumed body unterminates at the line-3 LF: pinned to the last unit
    // before it (the `.` at byte 35, analyzer endOffset-1 law).
    assert_eq!(d0.range.offset, 35);
    assert_eq!(d0.range.length, 1);
    // The `");` tail scans as a fresh string and unterminates at the line-4
    // LF (pinned to the `;` at byte 61).
    assert_eq!(d1.range.offset, 61);
    assert_eq!(d1.range.length, 1);
    // Pairs: the discarded `${` (synthetic `}` at the `$`, byte 30) and
    // print's `(` (synthetic `)` at the line-5 `}`, byte 65 — it matches
    // deep below m1's brace).
    let t = &r.tokens;
    let pairs = t.unmatched_groups();
    assert_eq!(pairs.len(), 2, "{pairs:?}");
    let sii = t
        .as_slice()
        .iter()
        .position(|tok| tok.kind == K::StringInterpolationExpression)
        .expect("SII token present");
    let sii_syn = t.matching_end(sii).expect("SII closed by recovery");
    assert_eq!(t.get(sii_syn).unwrap().byte_offset, 30);
    let paren_syn = pairs
        .iter()
        .find(|&&(begin, _)| t.get(begin as usize).unwrap().kind == K::OpenParen)
        .map(|&(_, close)| close)
        .expect("the print `(` pair");
    assert_eq!(t.get(paren_syn as usize).unwrap().byte_offset, 65);
    assert_eq!(t.get(paren_syn as usize).unwrap().kind, K::CloseParen);
}

/// Negative: when the misaligned brace's closer sits at the SAME indentation
/// the walk declines (official `-1`/equal-indent guards) and no re-scan
/// happens — the missing `}` stays parked at eof as an unmatched pair (the
/// parser reports it zero-width at the eof offset).
#[test]
fn same_indent_closer_does_not_rescan() {
    let src = "class C {\n  void m() {\n    if (a) {\n    }\n  }\n  void m2() {}\n";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    let pairs = t.unmatched_groups();
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    let (begin, close) = pairs[0];
    assert_eq!(t.get(begin as usize).unwrap().kind, K::OpenCurly);
    assert!(t.get(close as usize).unwrap().is_synthetic());
    assert_eq!(
        t.get(close as usize).unwrap().byte_offset,
        u32_of(src.len())
    );
}

/// Negative: two unclosed braces at EOF never take the re-scan (official
/// gate requires exactly one `{` on the stack); both pairs park at eof.
#[test]
fn two_unclosed_braces_do_not_rescan() {
    let src = "class C {\n  void m() {\n    if (a) {\n  }\n";
    let r = scan_src(src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let t = &r.tokens;
    let pairs = t.unmatched_groups();
    assert_eq!(pairs.len(), 2, "one per unclosed brace, at eof");
    for &(begin, close) in pairs {
        assert_eq!(t.get(begin as usize).unwrap().kind, K::OpenCurly);
        let closer = t.get(close as usize).unwrap();
        assert!(closer.is_synthetic());
        assert_eq!(closer.byte_offset, u32_of(src.len()));
    }
}

// --- the handed-off corpus files themselves (tracked in tests/corpus) ---
//
// Scanner-level oracle pins (`dart analyze --format=machine`, Dart 3.15
// main): the re-scan reduces every file in the #440 handoff to exactly the
// oracle's scanner diagnostics — class_in_class / named_function_expression
// / the extra unterminated_string_literal are parser-side over-reports that
// disappear once the token stream carries the recovered shape.

fn corpus_file(rel: &str) -> Option<String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/m0-syntax/lib/src/error_recovery/")
        .join(rel);
    let src = std::fs::read_to_string(&path).ok()?;
    assert!(!src.is_empty(), "{rel} unexpectedly empty");
    Some(src)
}

#[test]
fn corpus_typing_files_recover_to_single_expected_token() {
    // oracle: expected_token '}' @ the recovery closer, len 1 — one per file,
    // nothing else scanner-level. Scanner-side: exactly one unmatched pair,
    // its synthetic closer parked at the recovery closer; the row reports
    // post-parse.
    let cases = [
        ("with_outline/typing_01.dart", "\n  }\n"),
        ("with_outline/typing_03.dart", "\n        }\n"),
        ("with_outline/typing_04.dart", "\n        }\n"),
        ("with_outline/typing_05.dart", "\n        }\n"),
    ];
    for (rel, closer_pattern) in cases {
        let Some(src) = corpus_file(rel) else {
            eprintln!("skip: {rel} not present");
            continue;
        };
        let r = scan_src(&src);
        let closer = src
            .find(closer_pattern)
            .unwrap_or_else(|| panic!("{rel}: closer pattern {closer_pattern:?} not found"))
            + closer_pattern.len()
            - 2;
        assert!(r.diagnostics.is_empty(), "{rel}: {:?}", codes(&r));
        let t = &r.tokens;
        let pairs = t.unmatched_groups();
        assert_eq!(pairs.len(), 1, "{rel}: {pairs:?}");
        let (begin, close) = pairs[0];
        assert_eq!(t.get(begin as usize).unwrap().kind, K::OpenCurly, "{rel}");
        let syn = t.get(close as usize).unwrap();
        assert!(syn.is_synthetic(), "{rel}");
        assert_eq!(syn.byte_offset, u32_of(closer), "{rel}: recovery closer");
        assert_eq!(syn.kind, K::CloseCurly, "{rel}");
    }
}

#[test]
fn corpus_string_interpolation_1_recovers() {
    let Some(src) = corpus_file("with_outline/typing_string_interpolation_1.dart") else {
        eprintln!("skip: corpus file not present");
        return;
    };
    let r = scan_src(src.as_str());
    // oracle: expected_token '}' @ the `$` — and NO unterminated_string_literal.
    // Scanner-side: no diagnostics, one unmatched pair at the `$`.
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    let dollar = src.find("${").expect("the `${` shape");
    // The only synthetic `}` in the stream is the discarded `${`'s closer:
    // the print call and both class braces pair up with real tokens (no EOF
    // synthetic closers).
    let t = &r.tokens;
    let syn_closers: Vec<u32> = t
        .as_slice()
        .iter()
        .filter(|tok| tok.kind == K::CloseCurly && tok.is_synthetic())
        .map(|tok| tok.byte_offset)
        .collect();
    assert_eq!(syn_closers, [u32_of(dollar)]);
    let pairs = t.unmatched_groups();
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    let (begin, close) = pairs[0];
    assert_eq!(
        t.get(begin as usize).unwrap().kind,
        K::StringInterpolationExpression
    );
    assert_eq!(t.get(close as usize).unwrap().byte_offset, u32_of(dollar));
}

#[test]
fn corpus_string_interpolation_2_recovers() {
    let Some(src) = corpus_file("with_outline/typing_string_interpolation_2.dart") else {
        eprintln!("skip: corpus file not present");
        return;
    };
    let r = scan_src(src.as_str());
    // The scanner keeps only the two unterminated rows; both expected_token
    // rows ride the pair list (parser-side post-parse report).
    assert_codes(
        &r,
        &["unterminated_string_literal", "unterminated_string_literal"],
    );
    let dollar = src.find("${").expect("the `${` shape");
    let dot = dollar + 5; // the `.` ending line 11 (inside `${foo.`)
    let semi = src.rfind(");\n").expect("line-12 tail") + 1;
    let method_closer = src.find("\n  }\n").expect("method1 closer") + 3;
    let expected = [
        ("unterminated_string_literal", dot, 1),
        ("unterminated_string_literal", semi, 1),
    ];
    for (d, (code, offset, len)) in r.diagnostics.iter().zip(expected) {
        assert_eq!(d.code.name, code);
        assert_eq!(d.range.offset, u32_of(offset));
        assert_eq!(d.range.length, len);
    }
    // Pairs: the discarded `${` (synthetic `}` at the `$`) and the print `(`
    // (synthetic `)` at the method closer).
    let t = &r.tokens;
    let pairs = t.unmatched_groups();
    assert_eq!(pairs.len(), 2, "{pairs:?}");
    let sii = t
        .as_slice()
        .iter()
        .position(|tok| tok.kind == K::StringInterpolationExpression)
        .expect("SII token present");
    let sii_syn = t.matching_end(sii).expect("SII closed by recovery");
    assert_eq!(t.get(sii_syn).unwrap().byte_offset, u32_of(dollar));
    let paren_syn = pairs
        .iter()
        .find(|&&(begin, _)| t.get(begin as usize).unwrap().kind == K::OpenParen)
        .map(|&(_, close)| close)
        .expect("the print `(` pair");
    assert_eq!(
        t.get(paren_syn as usize).unwrap().byte_offset,
        u32_of(method_closer)
    );
    assert_eq!(t.get(paren_syn as usize).unwrap().kind, K::CloseParen);
}

/// `bracket_mismatch_01` (#440 deferral, re-measured under the dual-option
/// law): the second form's `]` mismatches `D(d:`'s paren one begin down, and
/// the drop posture wins strictly — insert = synthetic `)` + no-match `]` +
/// no-match `)` = 3 recoveries, drop = every closer pairs with its real
/// begin, +1 = 1 — so the scanner emits NOTHING at the `])` (upstream
/// `bracket_mismatch_01.dart.expect` reports only the parser-level pair
/// `Expected an identifier, but got ']'` / `Expected ')' before this`, both
/// at the `]);` — no scanner problem). No EOF diagnostics: the recovered
/// stream closes every remaining brace.
#[test]
fn corpus_bracket_mismatch_01_deep_match_drops() {
    let Some(src) = corpus_file("bracket_mismatch_01.dart") else {
        eprintln!("skip: corpus file not present");
        return;
    };
    let r = scan_src(&src);
    assert!(r.diagnostics.is_empty(), "{:?}", codes(&r));
    // Every begin pairs with a real closer; nothing synthetic anywhere.
    let t = &r.tokens;
    assert!(
        !t.as_slice()
            .iter()
            .any(super::super::token::Token::is_synthetic)
    );
    let bracket = src.rfind("]),").expect("the mismatched `])` of class D");
    let junk = t
        .as_slice()
        .iter()
        .find(|tok| tok.kind == K::CloseSquare && tok.byte_offset == u32_of(bracket))
        .expect("the mismatching `]` is a plain token");
    assert!(!junk.is_synthetic());
}
