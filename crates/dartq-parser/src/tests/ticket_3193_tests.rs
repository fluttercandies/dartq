//! #3193 pins: the end-of-entry `invalidInitializer` rejection and its
//! whole-entry span.
//!
//! Upstream runs a final validity check per initializer entry
//! (`endInitializers` → `buildInitializer`, ast_builder.dart:2209-2234): a
//! `super(...)`/`this(...)` call followed by `=`/`!`/another `(` leaves no
//! representable initializer node (`default: return null`), and the rejection
//! row anchors on `initializerObject.beginToken..endToken` — the WHOLE entry,
//! not the `super`/`this` head. The bare `super = 0` assignment takes the
//! same null path and the same whole-entry anchor.
//!
//! dartq decides the rejection in the dispatch lookahead (the parser has no
//! entry node graph) but defers the report until after the expression
//! re-read has fixed the entry end (`initializer_entry_span`, members.rs).

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

fn mas(start: u32, end: u32) -> (String, u32, u32) {
    code("missing_assignable_selector", start, end)
}

fn invalid_initializer(start: u32, end: u32) -> (String, u32, u32) {
    code("invalid_initializer", start, end)
}

/// `A() : super() = 0;` — the entry emitted nothing before #3193. The
/// assignment-to-invocation node is rejected wholesale over
/// `super() = 0` (16..27); the assignment arm's selector row on the
/// non-assignable `super()` target (16..23) survives (#3185).
#[test]
fn super_call_assignment_rejected_over_whole_entry() {
    assert_eq!(
        sorted_rows("class A { A() : super() = 0; }\n"),
        vec![invalid_initializer(16, 27), mas(16, 23)],
        "oracle: MAS on `super()` + invalidInitializer over the whole entry"
    );
}

/// `A() : this()();` — a chained invocation on a redirecting `this` call is
/// not a redirecting initializer: rejected wholesale over `this()()` (16..24),
/// no other rows (the expression itself parses silently).
#[test]
fn chained_this_call_rejected_over_whole_entry() {
    assert_eq!(
        sorted_rows("class A { A() : this()(); }\n"),
        vec![invalid_initializer(16, 24)],
        "oracle: one invalidInitializer row spanning `this()()`"
    );
}

/// `A() : super()!;` — the postfix `!` leaves no representable initializer
/// (issue_41597): the row moves from the bare `super` head to the whole
/// entry `super()!` (16..24).
#[test]
fn super_call_bang_rejected_over_whole_entry() {
    assert_eq!(
        sorted_rows("class A { A() : super()!; }\n"),
        vec![invalid_initializer(16, 24)],
        "oracle: one invalidInitializer row spanning `super()!`"
    );
}

/// `A() : super = 0;` — the bare-super anchor widens from the `super` head
/// (16..21) to the entry `super = 0` (16..25); the selector row on the
/// non-assignable target stays at the head (#3092, oracle 12:9|9).
#[test]
fn bare_super_assignment_anchors_on_whole_entry() {
    assert_eq!(
        sorted_rows("class A { A() : super = 0; }\n"),
        vec![invalid_initializer(16, 25), mas(16, 21)],
        "oracle: invalidInitializer spans `super = 0`, MAS stays on `super`"
    );
}

/// Control: a plain field initializer entry stays clean.
#[test]
fn plain_field_initializer_stays_clean() {
    assert_eq!(
        sorted_rows("class A { A() : x = 0; }\n"),
        vec![],
        "oracle: a valid initializer entry emits no rows"
    );
}
