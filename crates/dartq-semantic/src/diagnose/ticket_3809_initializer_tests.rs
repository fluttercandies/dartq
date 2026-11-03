//! The #3809 pins — `initializer_for_non_existent_field`, the synthetic-name
//! cluster.
//!
//! # What the oracle reports
//!
//! An initializer item whose head token is neither an identifier nor one of
//! the three keyword arms (`assert`, `this`, `super`) makes the parser insert
//! a whole synthetic assignment: a zero-length field name at the head token,
//! then `=`, then a synthetic expression (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`,
//! `parseInitializer`'s fallthrough — `insertSyntheticIdentifier(..., diag.
//! expectedAnInitializer)`). The analyzer's `_checkForInvalidField` walks that
//! empty spelling against the enclosing class's field set, finds no field
//! named `''`, and reports `initializer_for_non_existent_field` at the
//! zero-length node (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/error_verifier.dart`).
//!
//! Measured shapes (oracle `dart analyze` 3.13.7, each one verified against
//! the fixture below before the pin):
//!
//! | fixture head | reported position |
//! | --- | --- |
//! | `A() : var f;` | the `var` keyword |
//! | `B() : ;`, `C() : {}`, `D() :}` | the token after the `:` |
//! | `this:;` (primary-constructor body signature) | the `;` |
//! | `F() : new();` | the `new` keyword |
//!
//! All of them report `'' isn't a field in the enclosing class.` at
//! `length: 0`. Two heads keep the written name or report nothing at all:
//! `x = 0` (the ordinary item) and `f var f` (the identifier recovery, which
//! keeps `f` as the written name and reports no initializer row).
//!
//! The rig prefixes [`CORE_PRELUDE`] on one line, so every expected offset is
//! computed in the prefixed source — the fixtures below carry no line-number
//! arithmetic.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The `initializer_for_non_existent_field` rows of `source` as
/// `(offset, length, message)`, beside the rig's prefixed source.
fn initializer_rows(source: &str) -> (String, Vec<(u32, u32, String)>) {
    let report = report_ops(source);
    let prefixed = format!("{CORE_PRELUDE}{source}");
    let found = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "initializer_for_non_existent_field")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    (prefixed, found)
}

/// The offset of `needle`'s first occurrence in the prefixed source.
fn offset_of(prefixed: &str, source: &str, needle: &str) -> u32 {
    let in_source = source
        .find(needle)
        .unwrap_or_else(|| panic!("fixture does not spell {needle:?}"));
    u32::try_from(prefixed.len() - source.len() + in_source).expect("fixture fits u32")
}

/// The single synthetic-name row of a fixture that yields exactly one.
fn only_row(source: &str, needle: &str) -> (u32, u32, String) {
    let (prefixed, found) = initializer_rows(source);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one initializer row: {:?}",
        rows(&report_ops(source))
    );
    let row = found[0].clone();
    assert_eq!(
        row.0,
        offset_of(&prefixed, source, needle),
        "the row anchors at {needle:?}, not at the field spelling"
    );
    row
}

// —— A keyword head: `: var f` inserts the synthetic assignment. ——
#[test]
fn keyword_head_reports_the_empty_name_at_the_head_token() {
    let (_, length, message) = only_row("class A {A() : var f;}\n", "var f");
    assert_eq!(length, 0, "the synthetic name has no width: {message}");
    assert_eq!(message, "'' isn't a field in the enclosing class.");
}

// —— A statement-position head: `: ; var f` — the `;` is the head. ——
#[test]
fn semicolon_head_reports_the_empty_name_at_the_head_token() {
    let (_, length, message) = only_row("class B {B() : ; var f;}\n", "; var f");
    assert_eq!(length, 0);
    assert_eq!(message, "'' isn't a field in the enclosing class.");
}

// —— A body-opening brace head: `: {} var f` — the `{` is the head, and the
// —— empty map/set literal after it is part of the recovered body. ——
#[test]
fn body_brace_head_reports_the_empty_name_at_the_head_token() {
    let (_, length, message) = only_row("class C {C() : {} var f;}\n", "{} var f");
    assert_eq!(length, 0);
    assert_eq!(message, "'' isn't a field in the enclosing class.");
}

// —— A close-curly head: `C() :}` — the class's own `}` follows the colon. ——
#[test]
fn close_curly_head_reports_the_empty_name_at_the_head_token() {
    let (_, length, message) = only_row("class D {D() :}\n", "}\n");
    assert_eq!(length, 0);
    assert_eq!(message, "'' isn't a field in the enclosing class.");
}

// —— A primary-constructor body signature's empty list: `this:;`. ——
#[test]
fn empty_primary_constructor_initializer_list_reports_the_empty_name() {
    let (_, length, message) = only_row("class E() {\n  this:;\n}\n", ";\n}\n");
    assert_eq!(length, 0);
    assert_eq!(message, "'' isn't a field in the enclosing class.");
}

// —— The primary-constructor abbreviation `new` in an initializer list
// —— (`augmenting_constructors_A11_t06` / `abbreviations_A01_t15`). ——
#[test]
fn new_keyword_head_reports_the_empty_name_at_the_keyword() {
    let (_, length, message) = only_row("class F {\n  F() : new();\n}\n", "new();");
    assert_eq!(length, 0);
    assert_eq!(message, "'' isn't a field in the enclosing class.");
}

// —— Negative: the ordinary item keeps the written name and the whole node
// —— span (`x = 0`), never an empty spelling at `x`. ——
#[test]
fn identifier_head_keeps_the_written_name() {
    let (_, length, message) = only_row("class G {G() : x = 0;}\n", "x = 0");
    assert_eq!(length, 5, "the node runs name through expression");
    assert_eq!(message, "'x' isn't a field in the enclosing class.");
}

// —— Negative: the identifier recovery `f var f` keeps `f` as the written
// —— name; the oracle reports no initializer row for it (measured), so a
// —— synthesized empty name here would be an extra. ——
#[test]
fn identifier_recovery_head_reports_no_initializer_row() {
    let (_, found) = initializer_rows("class H {H() : f var f;}\n");
    assert!(
        found.is_empty(),
        "the identifier arm owns its own recovery: {:?}",
        rows(&report_ops("class H {H() : f var f;}\n"))
    );
}
