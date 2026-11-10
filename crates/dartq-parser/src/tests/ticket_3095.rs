//! #3095 pins: the bare-`this` assignment target's selector row in
//! non-statement positions — upstream's assignment arm emits
//! `missingAssignableSelector` unconditionally for the non-assignable
//! target (`handleAssignmentExpression`, ast_builder.dart:3831-3836;
//! `_isAssignable` :6607 admits only names, property/index extractions and
//! dot shorthands), so `f(this = 3);`, `var x = this = 3;`,
//! `for (this = 3;;)`, and `C() : x = this = 3;` all carry the row on the
//! `this` token (oracle 3.13.1).
//!
//! The redirecting-initializer lane stays silent:
//! `C() : this = 0;` carries expected_token +
//! missing_assignment_in_initializer and never the selector row — that
//! entry head is consumed by the initializer parser's own recovery, so no
//! this-target assignment reaches the arm from it.

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

/// The bare-`this` selector row across the argument, initializer-value,
/// for-initializer, and constructor-initializer-value positions.
#[test]
fn nested_assignment_this_selector_rows_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { g(this = 3); }\n"),
        vec![mas(13, 17)],
        "oracle: the argument-position assignment target"
    );
    assert_eq!(
        sorted_rows("void f() { var x = this = 3; }\n"),
        vec![mas(19, 23)],
        "oracle: the nested initializer-value assignment target"
    );
    assert_eq!(
        sorted_rows("void f() { for (this = 3;;) {} }\n"),
        vec![mas(16, 20)],
        "oracle: the for-initializer assignment target"
    );
    assert_eq!(
        sorted_rows("class C { C() : x = this = 3; }\n"),
        vec![mas(20, 24)],
        "oracle: the constructor-initializer value's nested target — the \
         lane carries real assignments for expression heads"
    );
}

/// The redirecting-initializer silence face: `this = 0` as the entry head
/// degrades through the initializer recovery — expected_token on the `=`
/// follower and missing_assignment_in_initializer on the `this` — never
/// the selector row.
#[test]
fn redirecting_initializer_head_stays_silent() {
    assert_eq!(
        sorted_rows("class C { C() : this = 0; }\n"),
        vec![
            (String::from("expected_token"), 21, 22),
            (String::from("missing_assignment_in_initializer"), 16, 20),
        ],
        "oracle: no selector row on the redirecting-initializer head — \
         expected_token anchors on the `=` follower, the missing-assignment \
         row on the `this`"
    );
}
