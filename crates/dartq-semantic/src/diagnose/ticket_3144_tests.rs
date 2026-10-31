//! The #3144 pins: the `scopes_bodies.rs` staging faces shared by
//! `duplicate_definition`, `unused_local_variable`, and
//! `return_of_invalid_type`.
//!
//! Upstream citations:
//! - `duplicate_definition`'s statement-list scope: a block body, function
//!   body, or switch member's statement list owns ONE duplicate scope over
//!   `VariableDeclarationStatement`, `FunctionDeclarationStatement`, and
//!   `PatternVariableDeclarationStatement` elements
//!   (duplicate_definition_verifier.dart `checkStatements`, upstream Dart
//!   SDK source, pkg/analyzer/lib/src/error/duplicate_definition_verifier
//!   .dart:85-105); a for-head's declaration list owns its own
//!   (`checkForVariables`, :48-53); the formal list's scope is SEPARATE
//!   from the body scope (`_FormalParameterDuplicateIdentifierScope`,
//!   :56-82 — a body local never collides with a same-named formal);
//!   wildcards never collide (`isWildcard`, :350-360).
//! - `unused_local_variable`'s verdict: a tracked local reports when no
//!   READ lands on it (unused_local_elements_verifier.dart —
//!   `_isReadIdentifier`; a write-only or statement-discarded read never
//!   spares), a pattern-declaration group reports all-or-nothing
//!   (`visitPatternVariableDeclaration`), and the wildcard feature's
//!   all-underscores names spare (:985-994 — only the exact `_`, never
//!   `__`/`___`).
//! - `return_of_invalid_type`'s verdict: the innermost executable's
//!   latched return type decides `return e;` (return_type_verifier.dart
//!   `verifyReturnStatement`, :155-289); a bare `return;` reads `void`,
//!   whose only fit is the `void`/`dynamic` host (the `RETURN_WITHOUT_
//!   VALUE` arm is a separate code — :222-226).

use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_bare};

/// The `(offset, length)` pairs of one code's rows, source-ordered — the
/// fold's exact answer set for a shape, prelude-shifted.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// `(offset, length)` of the `nth` (0-based) occurrence of `needle` inside
/// `source`, shifted past [`CORE_PRELUDE`]. Fixtures are ASCII so byte and
/// UTF-16 offsets coincide.
fn at(source: &str, needle: &str, nth: usize) -> (u32, u32) {
    let found = source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing in {source:?}"));
    (
        found.0 as u32 + CORE_PRELUDE.len() as u32,
        needle.len() as u32,
    )
}

/// One span anchored at `offset` with `len`.
fn span(offset: u32, len: u32) -> (u32, u32) {
    (offset, len)
}

// ---------------------------------------------------------------------------
// duplicate_definition — the statement-list scope walk (dup_scope_add).
// ---------------------------------------------------------------------------

/// `checkStatements`: two `VariableDeclarationStatement`s of one block
/// scope collide at the second name token — the staging's
/// `dup_scope_add`/`stage_local_duplicate` path (the engine face's
/// `l1` bake owns only the element-model faces).
#[test]
fn block_two_named_locals_collide_at_second() {
    let report = report_ops("void f() {\n  int x;\n  int x;\n}\n");
    assert_eq!(
        rows_of(&report, "duplicate_definition"),
        vec![(CORE_PRELUDE.len() as u32 + 19, 1)],
        "second `x` anchors the row: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

/// Distinct names in one scope never collide — the same walk inserts each
/// name once and reports nothing.
#[test]
fn block_distinct_locals_stay_silent() {
    let report = report_ops("void f() {\n  int x;\n  int y;\n}\n");
    assert!(
        rows_of(&report, "duplicate_definition").is_empty(),
        "distinct names never collide: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

/// A formal parameter and a same-named body local live in separate scopes
/// (`_FormalParameterDuplicateIdentifierScope` vs `checkStatements`) — no
/// `duplicate_definition`, matching the oracle's silence.
#[test]
fn formal_and_local_same_name_stay_silent() {
    let report = report_ops("void f(int x) {\n  int x;\n}\n");
    assert!(
        rows_of(&report, "duplicate_definition").is_empty(),
        "formal scope is separate from the body scope: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

/// An inner block's same-named local shadows the outer — the sibling
/// statement list owns its own scope (`checkStatements` per block), so a
/// nested `int x` never collides with the outer `int x`.
#[test]
fn nested_block_shadow_stays_silent() {
    let report = report_ops("void f() {\n  int x;\n  {\n    int x;\n  }\n}\n");
    assert!(
        rows_of(&report, "duplicate_definition").is_empty(),
        "inner block owns a fresh scope: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

/// `checkForVariables`: the for-head's declaration list is its own scope —
/// `for (int i = 0, i = 0; ...)` collides at the second `i`.
#[test]
fn for_head_list_collides_at_second() {
    let report = report_ops("void f() {\n  for (int i = 0, i = 0; i < 5;) {}\n}\n");
    assert_eq!(
        rows_of(&report, "duplicate_definition"),
        vec![(CORE_PRELUDE.len() as u32 + 26, 1)],
        "the for-head list scope collides at the second `i`: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

/// The same-name local function and local variable collide inside one
/// statement-list scope (`FunctionDeclarationStatement` arm).
#[test]
fn local_variable_and_function_same_name_collide() {
    let report = report_ops("void f() {\n  int x;\n  void x() {}\n}\n");
    assert_eq!(
        rows_of(&report, "duplicate_definition"),
        vec![(CORE_PRELUDE.len() as u32 + 26, 1)],
        "variable + function same name collides: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

/// `checkStatements`' `PatternVariableDeclarationStatement` arm: a pattern
/// declaration's element joins the statement-list scope, so a following
/// `int a` collides with the pattern's `a`.
#[test]
fn pattern_element_then_local_collides() {
    let report = report_ops("void f() {\n  var (a, b) = (0, 1);\n  int a = 0;\n}\n");
    assert_eq!(
        rows_of(&report, "duplicate_definition"),
        vec![(CORE_PRELUDE.len() as u32 + 37, 1)],
        "pattern element joins the statement-list scope: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

/// Wildcards never collide (`isWildcard`): two `var _`-spelled locals in
/// one scope stay silent under the wildcard-variables feature.
#[test]
fn wildcard_locals_stay_silent() {
    let report = report_ops("void f() {\n  var _ = 0;\n  var _ = 1;\n}\n");
    assert!(
        rows_of(&report, "duplicate_definition").is_empty(),
        "`_` declares nothing under the feature: {:?}",
        rows_of(&report, "duplicate_definition")
    );
}

// ---------------------------------------------------------------------------
// unused_local_variable — the tracked-local verdict.
// ---------------------------------------------------------------------------

/// An unused typed local reports at its name token (the baseline face).
#[test]
fn single_unused_local_reports() {
    let report = report_ops("void f() {\n  int x;\n}\n");
    assert_eq!(
        rows_of(&report, "unused_local_variable"),
        vec![(CORE_PRELUDE.len() as u32 + 14, 1)],
        "the unused local reports at its name: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// A local whose name is read in a later statement is used — no row. The
/// `print` call is an official useful read.
#[test]
fn read_local_stays_silent() {
    let report = report_ops("void f() {\n  int x = 0;\n  print(x);\n}\n");
    assert!(
        rows_of(&report, "unused_local_variable").is_empty(),
        "a read local is used: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// Two differently-named unused locals each report — the close walks every
/// tracked row, never only the first.
#[test]
fn two_unused_locals_each_report() {
    let report = report_ops("void f() {\n  int x;\n  int y;\n}\n");
    assert_eq!(
        rows_of(&report, "unused_local_variable"),
        vec![
            (CORE_PRELUDE.len() as u32 + 14, 1),
            (CORE_PRELUDE.len() as u32 + 21, 1),
        ],
        "every unused tracked row reports: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// A used sibling never spares the unused one: `a`'s read spares only `a`.
#[test]
fn only_the_read_local_is_spared() {
    let report = report_ops("void f() {\n  int a;\n  int b;\n  print(a);\n}\n");
    assert_eq!(
        rows_of(&report, "unused_local_variable"),
        vec![(CORE_PRELUDE.len() as u32 + 21, 1)],
        "only `b` is unused: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// A statement-discarded read (`x;` alone) never spares — the official
/// `_isReadIdentifier`/`ExpressionStatement` discard. The bare `x;` reads
/// `x` but the statement window throws the read away.
#[test]
fn bare_statement_read_still_reports() {
    let report = report_ops("void f() {\n  int x = 0;\n  x;\n}\n");
    // `x;` is a discarded single-read statement — `check_statement_use`
    // drops the credit and `x` stays unused.
    assert_eq!(
        rows_of(&report, "unused_local_variable"),
        vec![(CORE_PRELUDE.len() as u32 + 14, 1)],
        "a discarded bare read never spares: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// `var (a, b)` — the `PatternVariableDeclaration` group reports
/// all-or-nothing: neither binder is read, so both report.
#[test]
fn unused_pattern_group_reports_each_binder() {
    let report = report_ops("void f() {\n  var (a, b) = (0, 1);\n}\n");
    assert_eq!(
        rows_of(&report, "unused_local_variable"),
        vec![
            (CORE_PRELUDE.len() as u32 + 15, 1),
            (CORE_PRELUDE.len() as u32 + 18, 1),
        ],
        "each unused pattern binder reports: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// The all-or-nothing group spares together: one read binder spares the
/// whole `PatternVariableDeclaration` — `print(a)` spares `b` too.
#[test]
fn used_pattern_binder_spares_the_group() {
    let report = report_ops("void f() {\n  var (a, b) = (0, 1);\n  print(a);\n}\n");
    assert!(
        rows_of(&report, "unused_local_variable").is_empty(),
        "one read binder spares the group: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// The wildcard exemption is exact-`_` only: `__` and `___` are ordinary
/// names and report when unused (`isWildcard` reads the element's whole
/// spelling, never a prefix).
#[test]
fn multi_underscore_names_report() {
    let report = report_ops("void f() {\n  var __ = 0;\n  var ___ = 0;\n}\n");
    assert_eq!(
        rows_of(&report, "unused_local_variable"),
        vec![
            (CORE_PRELUDE.len() as u32 + 14, 2),
            (CORE_PRELUDE.len() as u32 + 24, 3),
        ],
        "`__`/`___` are real names, not wildcards: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

/// `var _` alone declares nothing under the feature — no row.
#[test]
fn lone_wildcard_stays_silent() {
    let report = report_ops("void f() {\n  var _ = 0;\n}\n");
    assert!(
        rows_of(&report, "unused_local_variable").is_empty(),
        "the exact `_` is a wildcard: {:?}",
        rows_of(&report, "unused_local_variable")
    );
}

// ---------------------------------------------------------------------------
// return_of_invalid_type — the staged return verdict.
// ---------------------------------------------------------------------------

/// `return "s";` under an `int` host reports at the expression span.
#[test]
fn string_return_under_int_reports() {
    let report = report_ops("int f() {\n  return \"s\";\n}\n");
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![(CORE_PRELUDE.len() as u32 + 21, 3)],
        "String into int reports: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// `return 0;` under `int` is a match — silent.
#[test]
fn matching_int_return_stays_silent() {
    let report = report_ops("int f() {\n  return 0;\n}\n");
    assert!(
        rows_of(&report, "return_of_invalid_type").is_empty(),
        "int into int stays silent: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// `num` accepts `int` — the subtype leg spares.
#[test]
fn int_return_under_num_stays_silent() {
    let report = report_ops("num f() {\n  return 0;\n}\n");
    assert!(
        rows_of(&report, "return_of_invalid_type").is_empty(),
        "int into num stays silent: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// `return null;` under a non-nullable `int` reports (`Null` is not a
/// subtype of a non-nullable target).
#[test]
fn null_return_under_non_nullable_reports() {
    let report = report_ops("int f() {\n  return null;\n}\n");
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![(CORE_PRELUDE.len() as u32 + 21, 4)],
        "Null into int reports: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// `return null;` under `int?` is the nullable match — silent.
#[test]
fn null_return_under_nullable_stays_silent() {
    let report = report_ops("int? f() {\n  return null;\n}\n");
    assert!(
        rows_of(&report, "return_of_invalid_type").is_empty(),
        "Null into int? stays silent: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// `return x;` where `x` is `int?` under an `int` host reports — the
/// nullable source is not assignable to the non-nullable target.
#[test]
fn nullable_local_return_under_int_reports() {
    let report = report_ops("int f() {\n  int? x;\n  return x;\n}\n");
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![(CORE_PRELUDE.len() as u32 + 30, 1)],
        "int? into int reports: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// `Future<int>` async body returning `"s"` reports — the async verdict's
/// `flatten`/`futureValueType` projections answer the same question.
#[test]
fn async_future_int_returning_string_reports() {
    let report = report_ops_bare("Future<int> f() async {\n  return \"s\";\n}\n");
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![(CORE_PRELUDE.len() as u32 + 33, 3)],
        "String into Future<int>'s int reports: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// `Future<int>` SYNC body returning `"s"` reports — a `Future` host's
/// value is still a `Future`, so `String` never fits.
#[test]
fn sync_future_int_returning_string_reports() {
    let report = report_ops_bare("Future<int> f() {\n  return \"s\";\n}\n");
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![(CORE_PRELUDE.len() as u32 + 25, 3)],
        "String into Future<int> reports: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// A `void` host returning `0` reports — `int` is not `void`-assignable.
#[test]
fn int_return_under_void_reports() {
    let report = report_ops("void f() {\n  return 0;\n}\n");
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![(CORE_PRELUDE.len() as u32 + 22, 1)],
        "int into void reports: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// An expression-bodied `=>` function returning `"s"` under `int` reports
/// at the body's span (the `=>` head through the expression end).
#[test]
fn arrow_body_string_return_reports() {
    let report = report_ops("int f() => \"s\";\n");
    assert_eq!(
        rows_of(&report, "return_of_invalid_type"),
        vec![(CORE_PRELUDE.len() as u32 + 11, 3)],
        "=> body String into int reports: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}

/// A `void` host's bare `return;` is the legal early exit — silent.
#[test]
fn bare_return_under_void_stays_silent() {
    let report = report_ops("void f() {\n  return;\n}\n");
    assert!(
        rows_of(&report, "return_of_invalid_type").is_empty(),
        "bare return in void stays silent: {:?}",
        rows_of(&report, "return_of_invalid_type")
    );
}
