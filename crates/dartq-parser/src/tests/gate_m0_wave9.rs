//! M0-gate wave-9 regression pins (#681): the parser consumes the scanner's
//! two-pass missing-brace topology (scanner #495) without member-lane
//! cascades.
//!
//! The scanner's re-scan blames the misaligned `{`, pins a synthetic `}` at
//! the right closer, and the real `}` then closes the OUTER brace
//! (`abstract_scanner.dart:488-514`; the dartq face is pinned in
//! dartq-scanner `curly_recovery.rs`). This suite pins the parser side: in
//! the recovered stream every block/body close rides the topology — the
//! statement loops stop at the synthetic closer (the first `}` in stream
//! order), `ensure_block`/`skip_to_token` ride `TokenTable::matching_end`,
//! and `ensure_semicolon` inserts without eating the real `}` — so a
//! sibling class after a broken member stays TOP-LEVEL (oracle
//! `error_recovery/with_outline/typing_0{1,3,4,5}.dart`: exactly one
//! scanner `expected_token`, zero parser diagnostics, no `class_in_class`).
//!
//! The r5 differential rows that motivated this lane (gate-caliber-r5,
//! 2026-10-10 15:33) were produced by the PRE-#495 scanner stream (missing
//! braces pinned at EOF); on the recovered topology the parse is clean, and
//! these tests lock that face. The prime2 control at the bottom keeps the
//! genuine member-position `class_in_class` gate intact — the correct fix
//! for a stale stream is never "report less".

use super::collector::Collector;
use crate::parser::Parser;

/// Scans `source` (scanner-flagged inputs allowed) and parses the recovered
/// table at the corpus language version; returns the parser listener plus
/// the scanner diagnostics as `(code, utf16 offset, length)`.
fn parse_with_scanner(source: &'static str) -> (Collector, Vec<(String, u32, u32)>) {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let scanner: Vec<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| (d.code.name.to_owned(), d.range.offset, d.range.length))
        .collect();
    let features =
        crate::ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(3, 13),
        ));
    let mut parser = Parser::new(text, table, features, Collector::default());
    parser.parse_unit();
    (parser.listener, scanner)
}

/// Offset of the recovery closer: the first real `}` after the
/// `// missing }` comment marker (the comment's own `}` is not a token).
fn recovery_closer(src: &'static str) -> u32 {
    let comment_end = src.find("// missing }").unwrap() + "// missing }".len();
    (src[comment_end..].find('}').unwrap() + comment_end) as u32
}

/// Asserts the shared sibling-class outcome: the two methods land inside
/// Foo and `Bar` begins its own top-level declaration after Foo ends.
fn assert_sibling_class_top_level(c: &Collector) {
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("begin", "method"),
        ("end", "method"),
        ("begin", "method"),
        ("end", "method"),
        ("end", "body"),
        ("end", "class_declaration"),
        ("begin", "class_declaration"),
        ("begin", "body"),
        ("end", "body"),
        ("end", "class_declaration"),
    ]);
}

/// typing_01 shape: the `if {` the user just typed stays open, so pass 1
/// closes it with the method's `}` and leaves the class brace dangling at
/// EOF. The re-scan pins the synthetic `}` at the misaligned closer, the
/// real `}` closes the method, and the next real `}` closes the class —
/// `foo_method2` must land as Foo's SECOND member and `class Bar` must
/// parse at top level with zero parser diagnostics (the oracle reports
/// only the scanner's one `expected_token`).
#[test]
fn two_pass_recovered_closer_realigns_member_loop() {
    let src = "class Foo {\n  void foo_method1() {\n    if (a) { // missing }\n  }\n\n  void foo_method2() {}\n}\n\nclass Bar {}\n";
    let (c, scanner) = parse_with_scanner(src);
    assert_eq!(
        scanner,
        [("expected_token".to_owned(), recovery_closer(src), 1)],
        "scanner face: one expected_token at the misaligned closer"
    );
    assert!(
        c.errors.is_empty(),
        "the recovered topology must parse clean: {:?}",
        c.errors
    );
    assert_sibling_class_top_level(&c);
}

/// typing_03 shape: the typed-open brace sits three ifs deep; the re-scan
/// still blames it (its closer is shallower-indented), the closers realign
/// one level each, and the member loop resumes at `foo_method2` with Bar
/// top-level.
#[test]
fn two_pass_deep_nested_typed_open_realigns_member_loop() {
    let src = "class Foo {\n  void foo_method1() {\n    if (a) {\n      if (a) {\n        if (b) { // missing }\n      }\n    }\n  }\n\n  void foo_method2() {}\n}\n\nclass Bar {}\n";
    let (c, scanner) = parse_with_scanner(src);
    assert_eq!(
        scanner,
        [("expected_token".to_owned(), recovery_closer(src), 1)],
        "scanner face: one expected_token at the misaligned closer"
    );
    assert!(
        c.errors.is_empty(),
        "the recovered topology must parse clean: {:?}",
        c.errors
    );
    assert_sibling_class_top_level(&c);
}

/// typing_04/05 shape: the unclosed brace is a map literal (`= {` branch of
/// the indent heuristic). The synthetic closer ends the literal, and the
/// statement's missing `;` reports ONCE (`ensureSemicolon`; the oracle
/// carries the same row) without eating the real `}` that now closes the
/// if — so the closers realign and Bar stays top-level.
#[test]
fn two_pass_unclosed_map_literal_keeps_sibling_class_top_level() {
    let src = "class Foo {\n  void m() {\n    if (a) {\n      var x = {1: 2 // missing }\n    }\n  }\n}\n\nclass Bar {}\n";
    let (c, scanner) = parse_with_scanner(src);
    assert_eq!(
        scanner,
        [("expected_token".to_owned(), recovery_closer(src), 1)],
        "scanner face: one expected_token at the map's recovery closer"
    );
    assert_eq!(
        c.error_codes(),
        ["expected_token"],
        "parser face: exactly the missing-`;` row, no member-lane cascade: {:?}",
        c.errors
    );
    c.assert_sequence(&[
        ("handle", "literal_set_or_map"),
        ("end", "body"),
        ("end", "class_declaration"),
        ("begin", "class_declaration"),
        ("begin", "body"),
        ("end", "body"),
        ("end", "class_declaration"),
    ]);
}

/// prime2 positive control (error_recovery/method_called_with_prime2.dart,
/// official intertwined expect :113): a `class` keyword that GENUINELY sits
/// at member position keeps `reportAndSkipClassInClass` — report,
/// `handleInvalidMember`, silent skip of the nested shape (no class events
/// for M2). The topology pins above must never silence this gate.
#[test]
fn member_position_class_keeps_class_in_class_report() {
    let mut p = super::collector::setup("class M1 {\nfoo\nclass M2 {\n}\n}\n");
    p.parse_unit();
    let c = p.listener;
    assert_eq!(
        c.error_codes(),
        [
            "missing_const_final_var_or_type",
            "expected_token",
            "class_in_class"
        ],
        "{:?}",
        c.errors
    );
    c.assert_sequence(&[
        ("begin", "class_declaration"),
        ("end", "body"),
        ("end", "class_declaration"),
    ]);
}
