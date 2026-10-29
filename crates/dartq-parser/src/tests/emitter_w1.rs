//! Emitter-wave W1 pins (#862): the native-family repoints and the
//! extension-type trailing-comma dead verdict, each probe-locked against
//! the pinned oracle (dart analyze 3.13.1; machine rows recorded verbatim
//! at the emit sites and in the corpus fixture headers).
//!
//! * `native_clause_in_non_sdk_code` — clause span runs from the `native`
//!   keyword through the closing quote when named (`native "corpus"` len
//!   15; bare `native` len 6). The old parse-tier
//!   `native_clause_should_be_annotation` report never surfaces in oracle
//!   output (probed silent on both formats) and is retired.
//! * `native_function_body_in_non_sdk_code` — body span runs from the
//!   keyword through the semicolon (`native "corpus";` len 16); without
//!   the semicolon no `NativeFunctionBody` node is built and the oracle's
//!   only row is `missing_function_body`.
//! * `representation_field_trailing_comma` — dead at 3.13: no analyzer
//!   emitter, probed silent; dartq must not manufacture a row.

use super::collector::{Collector, latest_features, scan_ok, setup};
use crate::Parser;

/// Sorted diagnostic-code multiset of one whole-unit parse.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    p.parse_unit();
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

/// Asserts the exact diagnostic multiset (codes only, sorted).
fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let mut expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

/// Asserts `code` never fires — the probe-first "recorded dead" pin (the
/// oracle face is empty, so any dartq row is gate-negative).
fn assert_code_absent(source: &'static str, code: &str, label: &str) {
    let mut p = setup(source);
    p.parse_unit();
    let got = p.listener.error_codes();
    assert!(
        !got.iter().any(|c| c == code),
        "{label}: {code} fired: {got:?}"
    );
}

/// Parses and returns the first diagnostic's `(code, start, end)` span
/// (UTF-16 units; ASCII fixtures make them byte-exact with the probes).
fn first_span(source: &'static str) -> (String, u32, u32) {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut p = Parser::new(text, table, latest_features(), &mut collector);
    p.parse_unit();
    let (code, start, end) = &p.listener.errors[0];
    (code.clone(), *start, *end)
}

// —— native clause (class header) ————————————————————————————————————————

#[test]
fn native_class_clause_reports_non_sdk_code() {
    assert_codes(
        "class NativeClass native \"corpus\" {}\n",
        &["native_clause_in_non_sdk_code"],
        "named native class clause",
    );
}

#[test]
fn native_class_clause_bare_keyword() {
    assert_codes(
        "class Bare native {}\n",
        &["native_clause_in_non_sdk_code"],
        "bare native class clause",
    );
}

/// Probed row: `min_native.dart` 1:19 len 15 — keyword start through the
/// closing quote.
#[test]
fn native_class_clause_span_matches_oracle_row() {
    let (code, start, end) = first_span("class C native \"corpus\" {}\n");
    assert_eq!(code, "native_clause_in_non_sdk_code");
    assert_eq!((start, end), (8, 23));
}

// —— native function body —————————————————————————————————————————————————

#[test]
fn native_function_body_reports_non_sdk_code() {
    assert_codes(
        "void nativeFunction() native \"corpus\";\n",
        &["native_function_body_in_non_sdk_code"],
        "native function body",
    );
}

/// Probed row: `min_native.dart` 2:23 len 16 — keyword start through the
/// semicolon.
#[test]
fn native_function_body_span_matches_oracle_row() {
    let (code, start, end) = first_span("void f() native \"corpus\";\n");
    assert_eq!(code, "native_function_body_in_non_sdk_code");
    assert_eq!((start, end), (9, 25));
}

/// Probed: without the semicolon the oracle builds no `NativeFunctionBody`
/// node — its only row is `missing_function_body`; neither native code may
/// fire and the retired `native_clause_should_be_annotation` must stay gone.
#[test]
fn native_body_without_semicolon_degrades_like_oracle() {
    let source = "void noSemi() native \"x\"";
    assert_code_absent(
        source,
        "native_function_body_in_non_sdk_code",
        "no-semicolon native body",
    );
    assert_code_absent(
        source,
        "native_clause_should_be_annotation",
        "no-semicolon native body",
    );
}

// —— manual-face reporting conventions (2026-10-29 wave) —————————————————

/// `(code, start, end)` rows for one whole-unit parse (UTF-16 units; the
/// ASCII fixtures make them byte-exact with the oracle rows).
fn rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut p = Parser::new(text, table, latest_features(), &mut collector);
    p.parse_unit();
    p.listener.errors.clone()
}

/// Manual face `literal_with_class_and_new/1` (oracle
/// `parser-fe/error_recovery/issue_45251_list_new.dart` 3:32|8): the row
/// spans `new` through the class identifier's end (`new List`), never the
/// bare keyword.
#[test]
fn literal_with_class_and_new_span_matches_oracle_row() {
    let (code, start, end) = first_span("void f() { var x = new List<int>[1]; }\n");
    assert_eq!(code, "literal_with_class_and_new");
    assert_eq!((start, end), (19, 27));
}

/// Manual face `type_parameter_on_constructor/1` (upstream
/// `outline_builder.dart` anchors at the type-parameter LIST node:
/// `typeParameters.charOffset` + `.spanLength`): `<T>` in full at 2:4|3.
#[test]
fn type_parameter_on_constructor_span_matches_oracle_row() {
    let source = "class C {\n  C<T>() {}\n}\n";
    let rows = rows(source);
    assert_eq!(
        rows,
        vec![("type_parameter_on_constructor".to_string(), 13, 16)],
        "{rows:?}"
    );
}

/// Manual face `type_parameter_on_operator/1`: `<T>` at 2:14|3.
#[test]
fn type_parameter_on_operator_span_matches_oracle_row() {
    let source = "class C {\n  operator []<T>(T t) => null;\n}\n";
    let rows = rows(source);
    assert_eq!(
        rows,
        vec![("type_parameter_on_operator".to_string(), 23, 26)],
        "{rows:?}"
    );
}

/// Manual face `factory_with_initializers/1`, message+span convention
/// halves: the registry texts carry no "but got" suffix
/// (`expectedClassMember`/`missingFunctionBody` are pseudo-shared codes the
/// analyzer converter re-renders with the fixed registry templates) and the
/// class-member/function-body rows anchor on the offending `:` (3:15|1).
/// The `missing_method_parameters` span half (oracle 3:23|1 through the
/// `findNonZeroLengthToken` walk, pinned live at the emit site) is owned by
/// the member-recovery shape: the in-flight recovery rework currently hands
/// that site a real `return` name with a three-row cascade, a
/// recovery-ordering divergence outside this conventions wave — the walk
/// fires the moment the recovery hands it a zero-width synthetic name
/// again.
#[test]
fn factory_with_initializers_rows_match_the_oracle() {
    let source =
        "class C {\n  int x = 0;\n  factory C() : x = 1 {\n    return C._();\n  }\n  C._();\n}\n";
    let rows = rows(source);
    let (_, cm_start, cm_end) = rows
        .iter()
        .find(|(c, _, _)| c == "expected_class_member")
        .expect("expected_class_member row");
    assert_eq!((*cm_start, *cm_end), (37, 38), "{rows:?}");
    let (_, fb_start, fb_end) = rows
        .iter()
        .find(|(c, _, _)| c == "missing_function_body")
        .expect("missing_function_body row");
    assert_eq!((*fb_start, *fb_end), (37, 38), "{rows:?}");
    assert!(
        rows.iter()
            .any(|(c, _, _)| c == "missing_method_parameters"),
        "{rows:?}"
    );
}

/// The registry texts behind the same face (message halves of the
/// factory_with_initializers/1 pairs and the expected-string pair).
#[test]
fn registry_templates_carry_no_but_got_suffix() {
    let source =
        "class C {\n  int x = 0;\n  factory C() : x = 1 {\n    return C._();\n  }\n  C._();\n}\n";
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut p = Parser::new(text, table, latest_features(), &mut collector);
    p.parse_unit();
    assert!(
        p.listener
            .texts
            .contains(&"Expected a class member.".to_string()),
        "{:?}",
        p.listener.texts
    );
    assert!(
        p.listener
            .texts
            .contains(&"A function body must be provided.".to_string()),
        "{:?}",
        p.listener.texts
    );
    let (table, text) = scan_ok("part of;\n");
    let mut collector = Collector::default();
    let mut p = Parser::new(text, table, latest_features(), &mut collector);
    p.parse_unit();
    assert_eq!(p.listener.texts, ["Expected a string literal."]);
}

/// Manual face `named_function_type/1` (pinned oracle row
/// `diagnostics/invalid_inline_function_type/doc_0`: `int f(String s)` →
/// the inner `(` at 1:31|1): the anchor is the token opening the
/// parameter's own suffix (`beforeInlineFunctionType.next!`,
/// parser_impl.dart:2326), never the name, and the registry text names the
/// generic-function-type position.
#[test]
fn invalid_inline_function_type_anchors_on_the_suffix_opener() {
    let source = "typedef F = void Function(int g(int x));\n";
    let mut rows: Vec<_> = rows(source)
        .into_iter()
        .filter(|(c, _, _)| c == "invalid_inline_function_type")
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (_, start, end) = rows.remove(0);
    assert_eq!((start, end), (31, 32));
}

// —— representation_field_trailing_comma: recorded dead at 3.13 ——————————

/// Probed silent (`extension type E(int x,) {}` and the corpus shape both
/// produce zero rows at the pinned oracle): dartq must not emit the code.
#[test]
fn representation_field_trailing_comma_stays_dead() {
    assert_code_absent(
        "extension type E(int x,) {}\n",
        "representation_field_trailing_comma",
        "trailing comma in representation field",
    );
    assert_code_absent(
        "extension type TrailingComma(int x,) {}\n",
        "representation_field_trailing_comma",
        "trailing comma in representation field",
    );
}
