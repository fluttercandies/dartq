//! #3093 pins: the type-variable-list continuation after a spliced
//! synthetic name (oracle 3.13.1). When the TV head is a keep-set keyword,
//! the keep face splices a synthetic name and the list continuation must
//! swallow the tail cascade — the official `parseUnexpectedEnd` gate
//! (`type_info_impl.dart:1589-1602`) silences the `>`-cascade when the
//! completed type-variable token is synthetic (`token.isSynthetic`, i.e.
//! `range.length == 0`), so `void b<super int>()` carries only the
//! `missing_identifier` row and `void b<typedef>()` degrades to the same
//! single row. The bound-position face (`T extends ,` / `T extends super`)
//! anchors `expected_type_name` on the offending token.

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

/// The keep face followed by a non-closer tail: the synthetic name splices
/// and the `>`-cascade is silenced (no `expected_token '>'`).
#[test]
fn synthetic_name_continuation_swallows_the_cascade() {
    assert_eq!(
        sorted_rows("void b<super int>() {}\n"),
        vec![code("missing_identifier", 7, 12)],
        "oracle: only the keep-face row — the tail cascade is silenced"
    );
    assert_eq!(
        sorted_rows("void b<super>() {}\n"),
        vec![code("missing_identifier", 7, 12)],
        "oracle: the immediate-closer keep face, no cascade"
    );
    assert_eq!(
        sorted_rows("void b<extends int>() {}\n"),
        vec![code("missing_identifier", 7, 14)],
        "oracle: reserved keep head, silent tail"
    );
    assert_eq!(
        sorted_rows("void b<required int>() {}\n"),
        vec![code("missing_identifier", 7, 15)],
        "oracle: modifier keep head, silent tail"
    );
    assert_eq!(
        sorted_rows("void b<typedef>() {}\n"),
        vec![code("missing_identifier", 7, 14)],
        "oracle: the built-in keep head degrades to the single row — the \
         recognizer never rejects it"
    );
}

/// The bound-position face: a missing or non-type bound anchors
/// `expected_type_name` on the offending token.
#[test]
fn bound_position_faces_match_oracle() {
    assert_eq!(
        sorted_rows("class A<T extends, S> {}\n"),
        vec![code("expected_type_name", 17, 18)],
        "oracle: the missing bound anchors on the `,`"
    );
    assert_eq!(
        sorted_rows("void b<T extends super>() {}\n"),
        vec![code("expected_type_name", 17, 22)],
        "oracle: `super` is not a bound type"
    );
}

