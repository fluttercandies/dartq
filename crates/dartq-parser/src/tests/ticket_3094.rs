//! #3094 pins: the redirecting/field-initializer value selector —
//! `C() : x = super;` carries `missing_assignable_selector` on the `super`
//! value (oracle 3.13.1). The initializer entry's `= value` tail parses
//! through the shared expression engine
//! (`parse_initializer_expression_rest` → the assignment arm), so the row
//! rides the arm's existing `reportErrorIfSuper(rhs)` face
//! (ast_builder.dart:3653) — the pins lock the lane routing, the
//! per-entry scope (a plain `y = 1` entry stays silent), and the
//! repeat-entry shape.

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

fn mas(start: u32, end: u32) -> (String, u32, u32) {
    (String::from("missing_assignable_selector"), start, end)
}

/// The initializer-value face: one row per `= super` entry value, anchored
/// on the `super` token; plain values stay silent.
#[test]
fn initializer_value_selector_rows_match_oracle() {
    assert_eq!(
        sorted_rows("class C { C() : x = super; }\n"),
        vec![mas(20, 25)],
        "oracle: the initializer value face on the `super`"
    );
    assert_eq!(
        sorted_rows("class C { C() : x = super, y = 1; }\n"),
        vec![mas(20, 25)],
        "oracle: only the `super` value reports; the plain entry is silent"
    );
    assert_eq!(
        sorted_rows("class C { C() : x = super, y = super; }\n"),
        vec![mas(20, 25), mas(31, 36)],
        "oracle: one row per `= super` entry"
    );
}
