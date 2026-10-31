//! #3191 pins: the `annotation_on_type_argument` span ends at the
//! annotation's OWN last token, never at the follower.
//!
//! Upstream reports the row with `skipMetadata`'s return token as the end
//! (type_info_impl.dart:1441-1447):
//!
//! ```dart
//! next = skipMetadata(next);
//! parser.reportRecoverableErrorWithEnd(atToken, next, diag.annotationOnTypeArgument);
//! ```
//!
//! `skipMetadata` (util.dart:96) returns the metadata's own last token —
//! `A` for `@A`, `)` for `@a.b(1)`, the `>` of `@B<C>` — so the error covers
//! exactly the annotation. dartq used to peek AFTER the recompute, making
//! the span bleed into the follower token (the next `@` head or the type):
//! `@a @b` reported 6..10 instead of 6..8 — the ticket's 2-char overshoot
//! (ours 13/21 vs oracle 11/19).

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

fn aota(start: u32, end: u32) -> (String, u32, u32) {
    ("annotation_on_type_argument".to_string(), start, end)
}

/// Single annotation in a type-argument group: the row covers exactly
/// `@foo` (23..27), not the following type head `C`.
#[test]
fn ticket_3191_single_annotation_span_is_its_own_extent() {
    assert_eq!(
        sorted_rows("class C { m() => new A<@foo C>(); }\n"),
        vec![aota(23, 27)],
        "oracle: the error ends at the annotation's last token (`foo`), \
         never bleeding into the type"
    );
}

/// Two annotations: two rows, each exactly over its own metadata — the
/// first must NOT swallow the second annotation's `@` head, the second
/// must NOT swallow the type.
#[test]
fn ticket_3191_each_annotation_reports_its_own_span() {
    assert_eq!(
        sorted_rows("void f() { new A<@a @b int>(); }\n"),
        vec![aota(17, 19), aota(20, 22)],
        "oracle: `@a` = 17..19 and `@b` = 20..22; the follower `@`/`int` stays out"
    );
}

/// Qualified annotation with arguments: `skipMetadata` ends at the `)` of
/// `@a.b(1)`.
#[test]
fn ticket_3191_annotation_with_arguments_ends_at_close_paren() {
    assert_eq!(
        sorted_rows("void f() { new A<@a.b(1) int>(); }\n"),
        vec![aota(17, 24)],
        "oracle: `@a.b(1)` = 17..24; `int` stays out"
    );
}

/// Annotation with type arguments: `skipMetadata` ends at the group's `>`
/// (`@B<C>` = 17..22).
#[test]
fn ticket_3191_annotation_with_type_args_ends_at_group_closer() {
    assert_eq!(
        sorted_rows("void f() { new A<@B<C> int>(); }\n"),
        vec![aota(17, 22)],
        "oracle: `@B<C>` = 17..22; `int` stays out"
    );
}
