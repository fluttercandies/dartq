//! #3338 `expected_token` map-pattern key separator anchor pins.
//!
//! A map-pattern entry's key is a constant expression, and a modifier
//! keyword cannot head one: the official `parseExpression` stops on the
//! keyword without consuming it, so the missing `:` is reported ON the
//! modifier. Frozen oracle faces (co19 `Patterns/map_A03_t01`):
//!
//! ```text
//! {var a: 2}       -> expected to find ':' at `var`   (len 3)
//! {final b: 2}     -> expected to find ':' at `final` (len 5)
//! {final int c: 2} -> expected to find ':' at `final` (len 5)
//! {int d: 2}       -> expected to find ':' at `d`     (len 1)
//! ```
//!
//! The identifier head row is the regression face: it keeps the pre-existing
//! follower anchor, so the modifier special case cannot widen into the
//! ordinary key shapes. The pins cover the `:` rows only — that is the
//! separator contract this ticket changes; the entry loop's closer row
//! (`}` at the stray separator in a minimal `if (o case …)` shell) is a
//! separate, unpinned shape.

use super::collector::{Collector, latest_features, scan_ok};
use crate::parser::Parser;

const COLON: &str = "Expected to find ':'.";

/// Parses `source` and returns every recoverable row as
/// `(code, start_utf16, end_utf16, text)`.
fn rows(source: &'static str) -> Vec<(String, u32, u32, String)> {
    let (table, text) = scan_ok(source);
    let mut parser = Parser::new(
        text,
        table,
        latest_features(),
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    parser
        .listener
        .errors
        .iter()
        .zip(parser.listener.texts.iter())
        .map(|((code, start, end), text)| (code.clone(), *start, *end, text.clone()))
        .collect()
}

/// The map-pattern separator rows only (the face this ticket pins).
fn colon_rows(source: &'static str) -> Vec<(String, u32, u32, String)> {
    rows(source)
        .into_iter()
        .filter(|r| r.0 == "expected_token" && r.3 == COLON)
        .collect()
}

/// Offset of the first byte of `needle` in `source` (UTF-16 == byte offset
/// here: every pinned face is ASCII).
fn at(source: &str, needle: &str) -> u32 {
    u32::try_from(source.find(needle).expect("needle in source")).unwrap()
}

#[test]
fn var_key_anchors_on_the_modifier() {
    let source = "void f(Object o) { if (o case {var a: 2}) {} }";
    let key = at(source, "var a: 2");
    assert_eq!(
        colon_rows(source),
        vec![("expected_token".to_string(), key, key + 3, COLON.to_string())],
        "`{{var a: 2}}` must report the missing ':' on `var`, not on `a`"
    );
}

#[test]
fn final_key_anchors_on_the_modifier() {
    let source = "void f(Object o) { if (o case {final int c: 2}) {} }";
    let key = at(source, "final int c: 2");
    assert_eq!(
        colon_rows(source),
        vec![("expected_token".to_string(), key, key + 5, COLON.to_string())],
        "`{{final int c: 2}}` must report on `final`, not on `int`"
    );
}

#[test]
fn identifier_key_keeps_the_follower_anchor() {
    // Regression face: an identifier key IS a valid constant expression, so
    // the row keeps riding the token after it.
    let source = "void f(Object o) { if (o case {int d: 2}) {} }";
    let key = at(source, "d: 2");
    assert_eq!(
        colon_rows(source),
        vec![("expected_token".to_string(), key, key + 1, COLON.to_string())],
        "`{{int d: 2}}` must report on `d`"
    );
}

#[test]
fn well_formed_map_pattern_is_silent() {
    let source = "void f(Object o) { if (o case {1: var a}) { print(a); } }";
    assert_eq!(
        colon_rows(source),
        Vec::new(),
        "a well-formed map pattern reports no map-pattern separator row"
    );
}
