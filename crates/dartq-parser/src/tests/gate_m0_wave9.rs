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

// —— #646: unnamed-factory version gate + record-type tail faces ——————————

/// Parses `source` with the feature set of language version
/// `(major, minor)` and returns the sorted parser diagnostic codes.
fn sorted_codes_at(source: &'static str, major: u32, minor: u32) -> Vec<String> {
    let (table, text) = super::collector::scan_ok(source);
    let features =
        crate::ParserFeatures::from_feature_set(dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(major, minor),
        ));
    let mut parser = Parser::new(text, table, features, Collector::default());
    parser.parse_unit();
    let mut codes = parser.listener.error_codes();
    codes.sort();
    codes
}

/// #646 cluster 1: the unnamed-factory head is a FEATURE-GATED branch
/// (3.15 main: `isPrimaryConstructorsFeatureEnabled && next2.isA('(')` with
/// the clean-modifier gate, `parser_impl.dart:5465-5484`; the 91.0.0 anchor
/// has no such branch). The M0 corpus pairs
/// `primary_constructors/factory_constructor_head.dart` (default version,
/// feature on) with `factory_constructor_head_pre_feature.dart`
/// (`// @dart=3.10`, feature off), so the same source carries two oracle
/// faces:
///
/// * at 3.10 the `factory` built-in (`isIdentifier`) falls through the
///   member keyword chain into the method lane, where a `const` modifier
///   reports `const_method` (91.0.0 parseMethod tail :5569-5572) and an
///   `=` body reports `redirection_in_non_factory_constructor` — the
///   pre-feature residual face (const_method x4 + redirection x2).
/// * at 3.13 the clean-modifier `factory (` heads take
///   `parseFactoryMethod(hasName: false)`: only const BODIES report
///   `const_factory` (the body arm's `handleConstFactory`); redirections
///   and `external` shapes stay silent.
#[test]
fn unnamed_factory_head_is_feature_gated() {
    let src = "class C {\n  factory () => C();\n  factory () {}\n  factory () = C;\n  external const factory ();\n  const factory () => C();\n  const factory () {}\n  const factory () = C;\n  void factory() => C();\n}\n";
    assert_eq!(
        sorted_codes_at(src, 3, 10),
        [
            "const_method",
            "const_method",
            "const_method",
            "const_method",
            "redirection_in_non_factory_constructor",
            "redirection_in_non_factory_constructor",
        ],
        "pre-feature face: the method lane reports const/`=` shapes"
    );
    assert_eq!(
        sorted_codes_at(src, 3, 13),
        ["const_factory", "const_factory"],
        "default-version face: the factory lane reports const bodies only"
    );
}

/// #646 cluster 6: `record_type_one_positional_no_trailing_comma` fires
/// only on a genuine one-positional record TYPE (`type_parse.rs`, official
/// `computeRecordType`). The r5 rows in the valid flow-analysis corpus
/// files (`bug42653.dart`, `type_promotion/data/for.dart`) were pre-#495
/// stream cascades: a for-each header never enters record-type parsing, so
/// the recovered topology carries no record rows there.
#[test]
fn record_type_one_positional_face_and_for_each_control() {
    assert_eq!(
        sorted_codes_at("void f((int) x) {}\n", 3, 13),
        ["record_type_one_positional_no_trailing_comma"],
        "one-positional record type reports once"
    );
    assert!(
        sorted_codes_at("void f((int,) x) {}\n", 3, 13).is_empty(),
        "trailing comma keeps the record type silent"
    );
    assert!(
        sorted_codes_at("void f((int, int) x) {}\n", 3, 13).is_empty(),
        "two-positional record type stays silent"
    );
    assert!(
        sorted_codes_at(
            "void f(Object x) {\n  if (x is int) {\n    for (x in [0]) {\n      x;\n    }\n  }\n}\n",
            3,
            13
        )
        .is_empty(),
        "the for-each header never produces a record-type row"
    );
}
