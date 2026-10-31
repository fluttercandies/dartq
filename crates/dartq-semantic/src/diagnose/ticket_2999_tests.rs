//! The `unused_local_variable` write-vs-read credit law (#2999): upstream
//! `GatherUsedLocalElementsVisitor.visitSimpleIdentifier` credits a
//! `LocalVariableElement` only when `_isReadIdentifier` answers true
//! (unused_local_elements_verifier.dart:300-303 + :429-453) — an
//! identifier in getter context counts, EXCEPT a statement-position
//! (`parent.parent is ExpressionStatement`) `++`/`--` operand or a
//! non-`??=` compound/plain assignment LHS; `??=` counts everywhere and
//! every nested-position read counts.
//!
//! Oracle provenance: `dart analyze` 3.13.7 — the generated corpus rows
//! `test_variableDeclarationStatement_isRead_notUsed_{compoundAssign,
//! postfixExpr,prefixExpr}` (REPORT at the name token) and
//! `test_variableDeclarationStatement_isNullAssigned` (SILENT) pin the
//! statement-position verdicts verbatim.

use super::test_rig::{CORE_PRELUDE, assert_silent, code_names, report_ops, rows};

/// The `unused_local_variable` rows of `source`, as (offset, length)
/// pairs mapped back into `source` (the rig mounts [`CORE_PRELUDE`]
/// ahead of the fixture, so the report's ranges are absolute into the
/// mounted unit).
fn ulv(source: &str) -> Vec<(u32, u32)> {
    let base = CORE_PRELUDE.len() as u32;
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| (d.range.offset - base, d.range.length))
        .collect()
}

/// Declared and never touched — the baseline REPORT at the name token.
#[test]
fn declared_never_used_reports() {
    let source = "void f() { int l = 0; }";
    let at = source.find("l = 0").unwrap() as u32;
    assert_eq!(ulv(source), vec![(at, 1)]);
}

/// A plain read spares the declaration.
#[test]
fn read_spares() {
    let source = "void f() { int l = 0; print(l); }\nvoid print(int x) {}";
    assert_silent(&report_ops(source));
}

/// Statement-position compound assignment: the official
/// `_isReadIdentifier` refuses the LHS read (`parent.parent is
/// ExpressionStatement` and the operator is not `??=`), so a
/// write-only `l += 1;` still reports — oracle row
/// `test_variableDeclarationStatement_isRead_notUsed_compoundAssign`.
#[test]
fn compound_assign_statement_reports() {
    let source = "void f() { int l = 0; l += 1; }";
    let at = source.find("l = 0").unwrap() as u32;
    assert_eq!(ulv(source), vec![(at, 1)]);
}

/// A compound assignment in a NON-statement position reads the LHS —
/// the discard is the ExpressionStatement arm only — so `(l += 1)` as
/// an argument spares the declaration.
#[test]
fn compound_assign_nested_spares() {
    let source = "void f() { int l = 0; print(l += 1); }\nvoid print(int x) {}";
    assert_silent(&report_ops(source));
}

/// Postfix `l++;` in statement position is not a read — oracle row
/// `test_variableDeclarationStatement_isRead_notUsed_postfixExpr`.
#[test]
fn postfix_increment_statement_reports() {
    let source = "void f() { int l = 0; l++; }";
    let at = source.find("l = 0").unwrap() as u32;
    assert_eq!(ulv(source), vec![(at, 1)]);
}

/// Prefix `++l;` in statement position is not a read — oracle row
/// `test_variableDeclarationStatement_isRead_notUsed_prefixExpr`.
#[test]
fn prefix_increment_statement_reports() {
    let source = "void f() { int l = 0; ++l; }";
    let at = source.find("l = 0").unwrap() as u32;
    assert_eq!(ulv(source), vec![(at, 1)]);
}

/// The decrement twins share the postfix/prefix arms — `l--;` / `--l;`
/// report the same way.
#[test]
fn decrement_statements_report() {
    let source = "void f() { int l = 0; l--; }\nvoid g() { int l = 0; --l; }";
    let f_at = source.find("l = 0").unwrap() as u32;
    let g_at = source.rfind("l = 0").unwrap() as u32;
    let mut got = ulv(source);
    got.sort_unstable();
    assert_eq!(got, vec![(f_at, 1), (g_at, 1)], "{got:?}");
}

/// `??=` reads its target even in statement position (the official
/// `_isReadIdentifier` `QUESTION_QUESTION_EQ` arm, oracle row
/// `test_variableDeclarationStatement_isNullAssigned`) — the variable
/// is spared.
#[test]
fn if_null_assign_statement_spares() {
    let source = "void f() { int? l; l ??= 5; }";
    assert_eq!(ulv(source), vec![], "rows: {:#?}", rows(&report_ops(source)));
}

/// `++`/`--` in a NON-statement position reads the variable upstream —
/// the for-updater's parent is the loop's updater list, never an
/// ExpressionStatement — so `for (; l < 10; l++) {}` spares the
/// declaration.
#[test]
fn for_updater_increment_spares() {
    let source = "void f() { int l = 0; for (; l < 10; l++) {} }";
    assert_eq!(ulv(source), vec![], "rows: {:#?}", rows(&report_ops(source)));
}

/// A plain `=` reassignment is never a read — the write-only life
/// reports even though the statement parses clean.
#[test]
fn plain_assign_statement_reports() {
    let source = "void f() { int l = 0; l = 5; }";
    let at = source.find("l = 0").unwrap() as u32;
    assert_eq!(ulv(source), vec![(at, 1)]);
}

/// The non-`??=` compound family all discard in statement position —
/// `l -= 1;`, `l *= 2;`, `l ~/= 2;` report identically.
#[test]
fn compound_family_statements_report() {
    let source = "void f() { int l = 0; l -= 1; }\n\
                  void g() { int l = 0; l *= 2; }\n\
                  void h() { int l = 0; l ~/= 2; }";
    let f_at = source.find("l = 0").unwrap() as u32;
    let g_at = source.find("l = 0; l *=").unwrap() as u32;
    let h_at = source.rfind("l = 0").unwrap() as u32;
    let mut got = ulv(source);
    got.sort_unstable();
    assert_eq!(got, vec![(f_at, 1), (g_at, 1), (h_at, 1)], "{got:?}");
}

/// A statement-position discard does not swallow a later genuine read
/// — `l += 1;` cancels its own read, `print(l);` re-spares the
/// declaration.
#[test]
fn discarded_compound_then_read_spares() {
    let source = "void f() { int l = 0; l += 1; print(l); }\nvoid print(int x) {}";
    assert_silent(&report_ops(source));
}

/// The bare-`l` statement stays a useful read — `l;` alone spares the
/// declaration upstream (`_isReadIdentifier`'s fall-through `true`).
#[test]
fn bare_identifier_statement_spares() {
    let source = "void f() { int l = 0; l; }";
    assert_eq!(ulv(source), vec![], "rows: {:#?}", rows(&report_ops(source)));
}

/// A write-only life through the for-in head: `for (l in xs)` assigns
/// `l` per iteration — the official `ForEachPartsWithIdentifier`
/// element is a write — so the loop mention never spares the
/// declaration.
#[test]
fn for_in_existing_variable_target_reports() {
    let source = "void f() { int l = 0; for (l in <int>[]) {} }";
    let at = source.find("l = 0").unwrap() as u32;
    assert_eq!(ulv(source), vec![(at, 1)]);
}

/// `await for (l in xs)` shares the write arm — the head's bare target
/// reports at the declaration.
#[test]
fn await_for_in_existing_variable_target_reports() {
    let source =
        "void f(Stream<int> s) async { int l = 0; await for (l in s) {} }";
    let at = source.find("l = 0").unwrap() as u32;
    assert_eq!(ulv(source), vec![(at, 1)]);
}

/// No `unused_local_variable` code lands when every local is read —
/// the code-name face of the silence, for the machine-format diff.
#[test]
fn no_unused_local_variable_code_when_all_read() {
    let source = "void f() { int l = 0; int m = l; print(m); }\nvoid print(int x) {}";
    assert!(
        !code_names(&report_ops(source)).contains(&"unused_local_variable"),
        "{:?}",
        rows(&report_ops(source))
    );
}
