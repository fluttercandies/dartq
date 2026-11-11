//! #3982 pins — the same-scope redeclaration binds FIRST (`LocalScope.add`
//! inserts through `_addGetter`'s `_getters[name] ??= element`, upstream
//! Dart SDK source pkg/analyzer/lib/src/dart/element/scope.dart): the
//! statement-list duplicate scope records the first declaration's range,
//! and [`Engine::resolve_local`] consults the innermost open scope's
//! stored range so a later read lands on the first declaration — never
//! the redeclaring sibling that shares the flat `locals` window. Every
//! expectation below is a verbatim oracle 3.13.7 row
//! (`dart analyze --format=machine`).

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::FileReport;

/// The `unused_local_variable` rows of `report`, as (offset, length)
/// pairs in source order.
fn unused_local_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The rows of `code` in `report`, as (offset, length) pairs in source
/// order.
fn code_rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The rig's prelude shift plus the byte offset of `context`'s `nth`
/// (0-based) occurrence in `fixture` — every fixture is ASCII, so the
/// UTF-16 and byte offsets coincide.
fn at(fixture: &str, context: &str, nth: usize) -> u32 {
    let prelude: usize = CORE_PRELUDE.chars().map(char::len_utf16).sum();
    let hit = fixture
        .match_indices(context)
        .nth(nth)
        .unwrap_or_else(|| panic!("fixture context missing: {context}#{nth}"))
        .0;
    (prelude + hit) as u32
}

/// Two declarations then a read: the read binds the FIRST `i`, so only
/// the duplicate reports `unused_local_variable`. Oracle:
/// `var i = 0; var i = 0; print(i);` → `unused_local_variable` at the
/// second `i` only (the #3152 pin's original defect — the flat window's
/// last-wins read credited the duplicate and flagged the first).
#[test]
fn same_scope_read_binds_first_declaration() {
    let fixture = "void f() {\n\
                   \x20 var i = 0;\n\
                   \x20 var i = 0;\n\
                   \x20 print(i);\n\
                   }\n\
                   void print(Object? o) {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        unused_local_rows(&report),
        vec![(at(fixture, "i = 0", 1), 1)],
        "rows: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name.to_string(), d.range.offset, d.range.length))
            .collect::<Vec<_>>()
    );
}

/// Three declarations then a read: the first still binds — both later
/// declarations report `unused_local_variable`, and both
/// `duplicate_definition` context rows anchor at the FIRST declaration
/// (upstream's context message reads "The first definition of this
/// name.", never the most recent repeat).
#[test]
fn triple_declaration_still_binds_first() {
    let fixture = "void f() {\n\
                   \x20 var i = 0;\n\
                   \x20 var i = 0;\n\
                   \x20 var i = 0;\n\
                   \x20 print(i);\n\
                   }\n\
                   void print(Object? o) {}\n";
    let report = report_ops(fixture);
    // The duplicate rows anchor at the second and third `i`.
    assert_eq!(
        code_rows(&report, "duplicate_definition"),
        vec![(at(fixture, "i = 0", 1), 1), (at(fixture, "i = 0", 2), 1)]
    );
    // Both redeclarations are unused; the first took the read.
    assert_eq!(
        unused_local_rows(&report),
        vec![(at(fixture, "i = 0", 1), 1), (at(fixture, "i = 0", 2), 1)]
    );
    // Every context row names the FIRST declaration.
    let context_offsets: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .flat_map(|d| {
            d.context
                .iter()
                .flat_map(|messages| messages.iter().map(|m| m.range.offset))
        })
        .collect();
    assert_eq!(
        context_offsets,
        vec![at(fixture, "i = 0", 0), at(fixture, "i = 0", 0)]
    );
}

/// A nested block's own declaration still shadows: the read inside the
/// block binds the INNER declaration (a different upstream scope), and
/// the outer declaration stays unused. First-wins never reaches across
/// scope boundaries — oracle reports the outer `i` only.
#[test]
fn nested_block_shadowing_still_binds_inner() {
    let fixture = "void f() {\n\
                   \x20 var i = 0;\n\
                   \x20 var i = 0;\n\
                   \x20 {\n\
                   \x20   var i = 0;\n\
                   \x20   print(i);\n\
                   \x20 }\n\
                   }\n\
                   void print(Object? o) {}\n";
    let report = report_ops(fixture);
    // The inner `i` is read; the body's two `i` declarations are unused
    // (the second is also the duplicate anchor).
    assert_eq!(
        unused_local_rows(&report),
        vec![(at(fixture, "i = 0", 0), 1), (at(fixture, "i = 0", 1), 1)]
    );
    assert_eq!(
        code_rows(&report, "duplicate_definition"),
        vec![(at(fixture, "i = 0", 1), 1)]
    );
}

/// A `for`-head redeclaration keeps the `dup_for_parts` face untouched:
/// `for (var i = 0, i = 1; ...)` still reports `duplicate_definition`
/// at the second name (the arm's `insert`-returned-old-value staging is
/// preserved). Oracle additionally binds the condition's read to the
/// FIRST `i` — the head's own `??=` map — which the flat-window read
/// does not model yet (the head map stores the last insert, and this
/// ticket's scope is the statement-list path only).
#[test]
fn for_head_redeclaration_reports_duplicate() {
    let fixture = "void f() {\n\
                   \x20 for (var i = 0, i = 1; i < 3;) {}\n\
                   }\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_rows(&report, "duplicate_definition"),
        vec![(at(fixture, "i = 1", 0), 1)]
    );
}
