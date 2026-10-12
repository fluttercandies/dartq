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
//!   the semicolon no NativeFunctionBody node is built and the oracle's
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
    let mut p = Parser::new(text, table, latest_features(), Collector::default());
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

/// Probed: without the semicolon the oracle builds no NativeFunctionBody
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
