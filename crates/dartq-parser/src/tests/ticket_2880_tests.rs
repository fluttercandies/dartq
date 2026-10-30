//! #2880 exact-row pins: the modifier-run walkers (`_parseModifiers`
//! always-consume-and-report semantics), the formal-parameter modifier
//! walk, the `.new` continuation rewrite over the full
//! `allowsNewAsIdentifier` context set, and the keyword-named method
//! block-body faces. Every expected row is the frozen oracle face
//! (`conformance/expected.tsv`) pinned as a full
//! `(line, column, length, code, text)` tuple; same-anchor multi-row sets
//! (the `(final external int i)` pair) are pinned WITHOUT dedup so count
//! regressions surface.

use super::collector::{Collector, latest_features, scan_ok, version_features};
use crate::ParserFeatures;
use crate::parser::Parser;

/// One expected oracle row, source-position based. `line`/`column` are the
/// 1-based TSV coordinates; `length` is the UTF-16 length (the pins are all
/// ASCII, so it matches the byte width).
type Row = (&'static str, u32, u32, u32, &'static str);

/// Parses `source` under `features` and returns the parser rows as
/// `(code, start, end, text)` sorted by position (gate_2791 harness shape).
fn faces_with(features: ParserFeatures, source: &'static str) -> Vec<(String, u32, u32, String)> {
    let (table, text) = scan_ok(source);
    let mut parser = Parser::new(
        text,
        table,
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    let mut rows: Vec<(String, u32, u32, String)> = parser
        .listener
        .errors
        .iter()
        .zip(parser.listener.texts.iter())
        .map(|((code, start, end), text)| (code.clone(), *start, *end, text.clone()))
        .collect();
    rows.sort_by(|a, b| (a.1, a.2, &a.0, &a.3).cmp(&(b.1, b.2, &b.0, &b.3)));
    rows
}

/// Converts 1-based `(line, column)` pins to offsets and compares the full
/// row set exactly.
fn assert_rows(source: &'static str, expected: &[Row], label: &str) {
    assert_rows_with(latest_features(), source, expected, label);
}

fn assert_rows_with(features: ParserFeatures, source: &'static str, expected: &[Row], label: &str) {
    let line_starts: Vec<u32> = std::iter::once(0u32)
        .chain(
            source
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i as u32 + 1),
        )
        .collect();
    let mut want: Vec<(String, u32, u32, String)> = expected
        .iter()
        .map(|(code, line, col, len, text)| {
            let start = line_starts[(*line - 1) as usize] + col - 1;
            ((*code).to_owned(), start, start + len, (*text).to_owned())
        })
        .collect();
    want.sort_by(|a, b| (a.1, a.2, &a.0, &a.3).cmp(&(b.1, b.2, &b.0, &b.3)));
    let got = faces_with(features, source);
    assert_eq!(got, want, "{label}: oracle face set diverged");
}

const DUP_AUGMENT: &str = "The modifier 'augment' was already specified.";
const CONFLICT_EXT_AUG: &str = "Members can't be declared to be both 'external' and 'augment'.";
const CONFLICT_AUG_EXT: &str = "Members can't be declared to be both 'augment' and 'external'.";
const AUG_BEFORE_STATIC: &str = "The modifier 'augment' should be before the modifier 'static'.";

/// P1: the full `augmentation/member_errors.dart` corpus face (parser tier).
/// Every duplicate/out-of-order/conflict modifier arm consumes its keyword
/// and reports exactly the oracle rows — the keyword never survives into the
/// member/name lane — with the augment/external pair as a
/// `conflicting_modifiers` face in both directions (upstream
/// `reportConflictingModifiers`, modifier_context.dart:431/:523).
#[test]
fn member_errors_modifier_run_rows() {
    let source = r#"class Class {
  augment augment method() {}
  augment external method();
  external augment method();

  augment augment void method() {}
  augment external void method();
  external augment void method();

  augment augment get getter => null;
  augment external get getter;
  external augment get getter;

  augment augment int get getter => 0;
  augment external int get getter;
  external augment int get getter;

  augment augment set setter(value) {}
  augment external set setter(value);
  external augment set setter(value);

  augment augment void set setter(value) {}
  augment external void set setter(value);
  external augment void set setter(value);

  augment augment var field;
  augment external var field;
  external augment var field;

  augment augment final field = 0;
  augment external final field;
  external augment final field;

  augment augment const field = 0;
  augment external const field;
  external augment const field;

  augment augment int field;
  augment external int field;
  external augment int field;

  augment augment late var field;
  augment late var field;
  augment late var field;

  augment augment late final field;
  augment late final field;
  augment late final field;

  augment augment late int field;
  augment late int field;
  augment late int field;

  augment augment static method() {}
  static augment method() {}

  augment augment static void method() {}
  static augment void method() {}

  augment augment static get getter => null;
  static augment get getter => null;

  augment augment static int get getter => 0;
  static augment int get getter => 0;

  augment augment static set setter(value) {}
  static augment set setter(value) {}

  augment augment static void set setter(value) {}
  static augment void set setter(value) {}

  augment augment static var field;
  static augment var field;

  augment augment static final field = 0;
  static augment final field = 0;

  augment augment static const field = 0;
  static augment const field = 0;

  augment augment static int field;
  static augment int field;

  augment augment static late var field;
  static augment late var field;

  augment augment static late final field;
  static augment late final field;

  augment augment static late int field;
  static augment late int field;
}
"#;
    // `duplicated_modifier` at the second `augment` of every doubled run
    // (corpus lines 2..90 even) and `modifier_out_of_order` at the
    // `augment` of every `static augment` run (:55..91) — the oracle emits
    // exactly one parser row per site, no riders.
    let dup_sites: &[u32] = &[
        2, 6, 10, 14, 18, 22, 26, 30, 34, 38, 42, 46, 50, 54, 57, 60, 63, 66, 69, 72, 75, 78, 81,
        84, 87, 90,
    ];
    let mut expected: Vec<Row> = dup_sites
        .iter()
        .map(|line| -> Row { ("duplicated_modifier", *line, 11, 7, DUP_AUGMENT) })
        .collect();
    // `augment external` conflicts on the `external` token, `external
    // augment` on the `augment` token — the member parses on in both
    // directions (:3-40, every member flavor).
    for line in [3u32, 7, 11, 15, 19, 23, 27, 31, 35, 39] {
        expected.push(("conflicting_modifiers", line, 11, 8, CONFLICT_EXT_AUG));
        expected.push(("conflicting_modifiers", line + 1, 12, 7, CONFLICT_AUG_EXT));
    }
    for line in [55u32, 58, 61, 64, 67, 70, 73, 76, 79, 82, 85, 88, 91] {
        expected.push(("modifier_out_of_order", line, 10, 7, AUG_BEFORE_STATIC));
    }
    // The parser-tier uninitialized-const rows on the external const fields
    // (corpus 35|26 and 36|26): the member parses ON after the conflict rows
    // — anchored on the `field` name, parseFieldInitializerOpt const arm.
    expected.push((
        "const_not_initialized",
        35,
        26,
        5,
        "The constant 'field' must be initialized.",
    ));
    expected.push((
        "const_not_initialized",
        36,
        26,
        5,
        "The constant 'field' must be initialized.",
    ));
    assert_rows(source, &expected, "member_errors modifier runs");
}

/// P2: `late` in every formal group position parses the parameter list on
/// and reports one `extraneous_modifier` on the keyword (oracle
/// `co19/LanguageFeatures/nnbd/static_errors_A21_t01` 15|5, 19|13, 23|13,
/// 27|18, 31|26 — the parameter loop restarts cleanly after the walk).
#[test]
fn formal_parameter_late_rows() {
    assert_rows(
        "class C {\n  C(late int x, int y) {}\n  C.n(int x, [late String y]) {}\n  C.m(int x, {late String y}) {}\n  static void s1(late int x) {}\n  void m1(late int x) {}\n}\n",
        &[
            (
                "extraneous_modifier",
                2,
                5,
                4,
                "Can't have modifier 'late' here.",
            ),
            (
                "extraneous_modifier",
                3,
                15,
                4,
                "Can't have modifier 'late' here.",
            ),
            (
                "extraneous_modifier",
                4,
                15,
                4,
                "Can't have modifier 'late' here.",
            ),
            (
                "extraneous_modifier",
                5,
                18,
                4,
                "Can't have modifier 'late' here.",
            ),
            (
                "extraneous_modifier",
                6,
                11,
                4,
                "Can't have modifier 'late' here.",
            ),
        ],
        "late in formal parameter groups",
    );
}

/// P2: `external` and `final external` in function-expression formals
/// (oracle `co19/.../external_variable_declaration_t04` 25|15, 30|15,
/// 30|21). The `(final external int i)` site carries TWO rows on the SAME
/// `external` anchor — the `_parseExternal` order chain row plus the
/// walk-end `extraneous_modifier` — pinned without dedup.
#[test]
fn formal_parameter_external_rows() {
    assert_rows(
        "test() {\n  var f3 = (external int i) {};\n  var f4 = (final external int i) {};\n}\n",
        &[
            (
                "extraneous_modifier",
                2,
                13,
                8,
                "Can't have modifier 'external' here.",
            ),
            (
                "extraneous_modifier",
                3,
                13,
                5,
                "Can't have modifier 'final' here.",
            ),
            (
                "extraneous_modifier",
                3,
                19,
                8,
                "Can't have modifier 'external' here.",
            ),
            (
                "modifier_out_of_order",
                3,
                19,
                8,
                "The modifier 'external' should be before the modifier 'final'.",
            ),
        ],
        "external in function-expression formals",
    );
}

/// P3a: `.new` in metadata continuations rewrites to the unnamed-constructor
/// identifier with no report (upstream `_tryRewriteNewToIdentifier` over the
/// `MetadataContinuation`/`MetadataContinuationAfterTypeArguments` contexts,
/// identifier_context_impl.dart:1112; oracle
/// `language/metadata/constructor_new_test` — zero rows).
#[test]
fn metadata_new_continuation_is_silent() {
    assert_rows(
        "class Class {\n  const Class();\n}\nclass GenericClass<X, Y> {\n  const GenericClass();\n}\n@Class.new()\n@GenericClass<int, String>.new()\nmain() {}\n",
        &[],
        "metadata .new continuations",
    );
}

/// P3a: `.new` primary-constructor heads (oracle
/// `Primary-constructors/static_processing_A15_t02` — zero rows; the head
/// face only parses under the feature flag, which gates the context).
#[test]
fn primary_constructor_new_head_is_silent() {
    assert_rows(
        "extension type ET1.new(int v);\nextension type ET2<T>.new(int v);\nclass C1.new(var int v);\nenum E1.new(final int v) {\n  e0(1);\n}\n",
        &[],
        "primary-constructor .new heads",
    );
}

/// P3b: built-in keywords name methods and calls silently (corpus
/// keyword_named_top_level_methods 239-247).
#[test]
fn builtin_named_method_is_clean() {
    assert_rows(
        "int part(int x) {\n  if (x == 0) return 42;\n  return part(x-1) + 1;\n}\n",
        &[],
        "built-in keyword method name",
    );
}

/// P3b: reserved `rethrow` reports once per name lane and the call face
/// consumes (corpus 251-254: 252|5 name, 254|10 call-site identifier).
#[test]
fn rethrow_named_method_rows() {
    assert_rows(
        "int rethrow(int x) {\n  if (x == 0) return 42;\n  return rethrow(x-1) + 1;\n}\n",
        &[
            (
                "expected_identifier_but_got_keyword",
                1,
                5,
                7,
                "'rethrow' can't be used as an identifier because it's a keyword.",
            ),
            (
                "expected_identifier_but_got_keyword",
                3,
                10,
                7,
                "'rethrow' can't be used as an identifier because it's a keyword.",
            ),
        ],
        "rethrow-named method",
    );
}

/// P3b: reserved `return` as the method name plus the
/// parsePrimary unexpected-`return` recovery at the inner call (corpus
/// 256-259: 257|5 name, 259|10 unexpected_token).
#[test]
fn return_named_method_rows() {
    assert_rows(
        "int return(int x) {\n  if (x == 0) return 42;\n  return return(x-1) + 1;\n}\n",
        &[
            (
                "expected_identifier_but_got_keyword",
                1,
                5,
                6,
                "'return' can't be used as an identifier because it's a keyword.",
            ),
            ("unexpected_token", 3, 10, 6, "Unexpected text 'return'."),
        ],
        "return-named method",
    );
}

/// P3b: `int switch(int x) { ... return switch(x-1) + 1; }` — the name lane
/// consumes the keyword with one report, the body still parses as a BLOCK,
/// and the inner `return switch(...)` recovers exactly like the oracle
/// (corpus 287-290): the send recovery leaves `switch` for the statement
/// loop (`missing_identifier` + synthetic), the return's semicolon row
/// anchors on `return` (the last real consumed token), the statement-loop
/// switch parses its scrutinee and anchors `expected_body` on the `)`, and
/// the leftover `+ 1;` takes the unary-`+` recovery. Pinned under the
/// corpus posture (patterns off — the oracle's recorded parse).
#[test]
fn switch_named_method_body_rows() {
    assert_rows_with(
        version_features(2, 19),
        "int switch(int x) {\n  if (x == 0) return 42;\n  return switch(x-1) + 1;\n}\n",
        &[
            (
                "expected_identifier_but_got_keyword",
                1,
                5,
                6,
                "'switch' can't be used as an identifier because it's a keyword.",
            ),
            ("expected_token", 3, 3, 6, "Expected to find ';'."),
            ("missing_identifier", 3, 10, 6, "Expected an identifier."),
            (
                "expected_body",
                3,
                20,
                1,
                "A switch statement must have a body, even if it is empty.",
            ),
            ("missing_identifier", 3, 22, 1, "Expected an identifier."),
        ],
        "switch-named method body",
    );
}
