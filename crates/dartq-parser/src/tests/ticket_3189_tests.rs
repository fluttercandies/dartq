//! #3189 pins: the `extends`/`implements`/`with`/`on` clause-list
//! named-type check.
//!
//! Upstream `_popNamedTypeList` (ast_builder.dart:6313-6326) walks every type
//! a clause list produced and reports `expected_named_type` over the type's
//! own span for every entry that is not a `NamedTypeImpl` — record types and
//! generalized function types are the two non-named shapes. The clause keeps
//! its own problem text (messages.yaml:23262-23289): extends "Expected a
//! class name.", implements/on "Expected the name of a class or mixin.",
//! with "Expected a mixin name.".
//!
//! dartq runs the check on the `TypeInfo` shape (`is_named_type` in
//! declarations.rs): `Simple`/`SimpleNullable`/`Prefixed`/`SimpleWith1Arg`
//! are named, `Complex` is named unless it is a top-level record type
//! (`is_record_type`) or a generalized function type (`gft_has_return_type`);
//! `NoType`/`void` keep their own recovery rows (`expected_type_name` /
//! `invalid_void`) and stay named here.

use super::collector::{latest_features, scan_ok};

struct Rows {
    rows: Vec<(String, u32, u32)>,
    texts: Vec<String>,
}

fn parse(source: &'static str) -> Rows {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    Rows {
        rows,
        texts: parser.listener.texts.clone(),
    }
}

fn ent(start: u32, end: u32) -> (String, u32, u32) {
    ("expected_named_type".to_string(), start, end)
}

const EXTENDS_TEXT: &str = "Expected a class name.";
const CLASS_OR_MIXIN_TEXT: &str = "Expected the name of a class or mixin.";
const MIXIN_TEXT: &str = "Expected a mixin name.";

fn text_of<'a>(parsed: &'a Rows, code: &str) -> Option<&'a str> {
    parsed
        .rows
        .iter()
        .position(|(c, _, _)| c == code)
        .map(|index| parsed.texts[index].as_str())
}

/// `class C extends (int, int) {}` — a record type is not a named type: one
/// row over the record span with the extends text.
#[test]
fn ticket_3189_extends_record_type() {
    let parsed = parse("class C extends (int, int) {}\n");
    assert_eq!(parsed.rows, vec![ent(16, 26)], "extends record span");
    assert_eq!(text_of(&parsed, "expected_named_type"), Some(EXTENDS_TEXT));
}

/// `class C implements (int, int) {}` — implements text.
#[test]
fn ticket_3189_implements_record_type() {
    let parsed = parse("class C implements (int, int) {}\n");
    assert_eq!(parsed.rows, vec![ent(19, 29)], "implements record span");
    assert_eq!(
        text_of(&parsed, "expected_named_type"),
        Some(CLASS_OR_MIXIN_TEXT)
    );
}

/// `class C with (int, int) {}` — mixin text.
#[test]
fn ticket_3189_with_record_type() {
    let parsed = parse("class C with (int, int) {}\n");
    assert_eq!(parsed.rows, vec![ent(13, 23)], "with record span");
    assert_eq!(text_of(&parsed, "expected_named_type"), Some(MIXIN_TEXT));
}

/// `mixin M on (int, int) {}` — on text (same wording as implements).
#[test]
fn ticket_3189_on_record_type() {
    let parsed = parse("mixin M on (int, int) {}\n");
    assert_eq!(parsed.rows, vec![ent(11, 21)], "on record span");
    assert_eq!(
        text_of(&parsed, "expected_named_type"),
        Some(CLASS_OR_MIXIN_TEXT)
    );
}

/// Only the non-named entry of a list reports; the named `A` stays silent.
#[test]
fn ticket_3189_multi_entry_list_reports_only_the_record() {
    let parsed = parse("class C implements A, (int, int) {}\n");
    assert_eq!(parsed.rows, vec![ent(22, 32)], "second entry only");
    assert_eq!(
        text_of(&parsed, "expected_named_type"),
        Some(CLASS_OR_MIXIN_TEXT)
    );
}

/// A generalized function type is the other non-named shape:
/// `class C extends void Function() {}` reports over the whole type.
#[test]
fn ticket_3189_extends_generalized_function_type() {
    let parsed = parse("class C extends void Function() {}\n");
    assert_eq!(parsed.rows, vec![ent(16, 31)], "gft span");
    assert_eq!(text_of(&parsed, "expected_named_type"), Some(EXTENDS_TEXT));
}

/// Named complex shapes stay silent: a type-argumented name and a prefixed
/// name both build `NamedTypeImpl` upstream.
#[test]
fn ticket_3189_named_complex_shapes_stay_silent() {
    assert!(
        parse("class C extends Map<String, int> {}\n")
            .rows
            .is_empty(),
        "type-argumented named type stays silent"
    );
    assert!(
        parse("class C extends a.B {}\n").rows.is_empty(),
        "prefixed named type stays silent"
    );
}

/// `void` runs the synthetic/named recovery (`invalid_void` row) and is not
/// a `expected_named_type` case.
#[test]
fn ticket_3189_void_is_not_a_named_type_miss() {
    let parsed = parse("class C extends void {}\n");
    assert!(
        parsed
            .rows
            .iter()
            .all(|(code, _, _)| code != "expected_named_type"),
        "void keeps expected_type_name only: {:?}",
        parsed.rows
    );
}

/// The named-mixin-application head (`class C = (int, int) with M;`) takes the
/// same extends face (`endNamedMixinApplication`, ast_builder.dart:2524).
#[test]
fn ticket_3189_named_mixin_application_head() {
    let parsed = parse("class C = (int, int) with M;\n");
    assert!(
        parsed.rows.contains(&ent(10, 20)),
        "named-mixin head record reports: {:?}",
        parsed.rows
    );
    assert_eq!(text_of(&parsed, "expected_named_type"), Some(EXTENDS_TEXT));
}
