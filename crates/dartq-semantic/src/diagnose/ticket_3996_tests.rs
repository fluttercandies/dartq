//! #3996/#3997 pins — the bare top-level-declaration write faces.
//!
//! #3996 (the `RefTarget::Decl` write face): a bare write, compound write,
//! increment, or for-in target whose push-time resolution landed on a
//! top-level declaration walks the oracle's setter question — `name=`
//! misses and the getter-side declaration reports `assignment_to_final`
//! at the written name (upstream Dart SDK source, pkg/analyzer/lib/src/
//! error/assignment_verifier.dart:63-87 — an explicit getter's variable is
//! a final property-inducing element, never a `FieldElement`, so the
//! plain `assignment_to_final` code wins, not `_no_setter`).
//!
//! #3997 (the compound lvalue's never-read face): a `x op= e` target is
//! resolved through `resolveForWrite` upstream, whose `setReadElement`
//! feeds `parent.readType` without `recordStaticType` — the read's
//! `Never` face never reaches `handleExit`, so the write's RHS runs LIVE.
//! The flow layer's `Ev::Assign` step restores the pre-exit state the
//! bottom-typed read parked (`compound_lvalue_saves`).
//!
//! Oracle provenance (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` on each fixture verbatim):
//! - `unreachable = 3` → `ASSIGNMENT_TO_FINAL|2|15|11` only.
//! - `unreachable += 3` → `ASSIGNMENT_TO_FINAL|2|15|11` only — NO dead row.
//! - `unreachable++` → `ASSIGNMENT_TO_FINAL` + `RECEIVER_OF_TYPE_NEVER` at
//!   the operand + `DEAD_CODE|2|30|15` (the postfix node itself exits).
//! - `unreachable ??= 3` → `ASSIGNMENT_TO_FINAL` + `DEAD_CODE|2|31|18` +
//!   `DEAD_NULL_AWARE_EXPRESSION|2|31|1`.
//! - `x += 3` over `void f(Never x)` → silent (no exit anywhere).
//! - `unreachable += nf()` over `Never nf()` → `ASSIGNMENT_TO_FINAL` +
//!   `DEAD_CODE` (the callee's own exit keeps the kill).
//!
//! Residuals (owners noted per face):
//! - `const`/function/type write targets own `assignment_to_const` /
//!   `assignment_to_function` / `assignment_to_type` — codes the enum does
//!   not carry (under-report direction).
//! - `x = 3` over `void f(Never x)` over-reports `dead_code` — the plain
//!   `=` local LHS has the same `resolveForWrite` no-read root (the
//!   `bare_plain_assign_member_target` gate skips only Decl/Member/Opaque
//!   targets).

use super::test_rig::{code_names, report_ops_bare, rows};
use crate::diagnose::FileReport;
use dartq_diagnostics::Diagnostic;

fn code_rows<'a>(report: &'a FileReport, code: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect()
}

/// #3996 — a plain `=` write to a top-level `Never` getter reports
/// `ASSIGNMENT_TO_FINAL` at the written name and nothing else. The #3991
/// no-dead-window face stays (the write target never read the getter's
/// `isBottom` face — the new write face only emits the diagnostic).
#[test]
fn plain_write_to_never_getter_reports_assignment_to_final() {
    let source = r#"Never get unreachable => throw 0;
void main() { unreachable = 3; }
"#;
    let report = report_ops_bare(source);
    let hits = code_rows(&report, "assignment_to_final");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    // The oracle's row: `unreachable` at line 2 col 15, length 11.
    assert_eq!(hits[0].range.offset, 48, "{:#?}", rows(&report));
    assert_eq!(hits[0].range.length, 11, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].message,
        "'unreachable' can't be used as a setter because it's final.",
        "{:#?}",
        rows(&report)
    );
    assert!(
        code_rows(&report, "dead_code").is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// #3996 — a write to a top-level `final` variable resolves its induced
/// `name=` miss and reports the same code (the variable's getter-only
/// face; `const` stays out — its `assignment_to_const` sibling code is
/// the under-report residual).
#[test]
fn write_to_top_level_final_var_reports_assignment_to_final() {
    let source = r#"final int x = 1;
void main() { x = 3; }
"#;
    let report = report_ops_bare(source);
    let hits = code_rows(&report, "assignment_to_final");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].message,
        "'x' can't be used as a setter because it's final.",
        "{:#?}",
        rows(&report)
    );
}

/// #3996 — the writable control: `var` induces `name=`, so the write
/// resolves the setter and reports nothing.
#[test]
fn write_to_top_level_var_stays_silent() {
    let source = r#"var x = 1;
void main() { x = 3; }
"#;
    let report = report_ops_bare(source);
    assert!(
        code_rows(&report, "assignment_to_final").is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// #3996 — the paired-setter control: an explicit `set unreachable`
/// answers the write (the getter+setter pair), so no finality verdict.
#[test]
fn write_to_getter_setter_pair_stays_silent() {
    let source = r#"Never get unreachable => throw 0;
set unreachable(Never v) {}
void main() { unreachable = throw 0; }
"#;
    let report = report_ops_bare(source);
    assert!(
        code_rows(&report, "assignment_to_final").is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// #3997 — a compound `+=` write to a top-level `Never` getter reports
/// the write face and NO dead row: upstream's `resolveForWrite` LHS never
/// records a static type, so `handleExit` never fires and the RHS runs
/// live (`unreachable += 3; print('after');` is fully live).
#[test]
fn compound_write_to_never_getter_reports_write_face_no_dead_row() {
    let source = r#"Never get unreachable => throw 0;
void main() { unreachable += 3; print('after'); }
"#;
    let report = report_ops_bare(source);
    let hits = code_rows(&report, "assignment_to_final");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        code_rows(&report, "dead_code").is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// #3997 — the local twin: `x += 3` over `void f(Never x)` is fully
/// silent upstream (the same `resolveForWrite` no-read root), so the
/// compound write revives the operand's bottom exit exactly like the
/// top-level face.
#[test]
fn compound_write_to_never_local_opens_no_dead_row() {
    let source = r#"void f(Never x) { x += 3; print('after'); }
"#;
    let report = report_ops_bare(source);
    assert!(
        code_rows(&report, "dead_code").is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// #3997 — the nested face: `x += (x += 1)` revives the inner write's
/// target into the outer's dead state, then the outer restores live —
/// oracle silent end to end.
#[test]
fn nested_compound_writes_to_never_getter_stay_live() {
    let source = r#"Never get unreachable => throw 0;
void main() { unreachable += (unreachable += 1); print('after'); }
"#;
    let report = report_ops_bare(source);
    // Both LHS writes resolve the getter — two `assignment_to_final` rows.
    assert_eq!(
        code_rows(&report, "assignment_to_final").len(),
        2,
        "{:#?}",
        rows(&report)
    );
    assert!(
        code_rows(&report, "dead_code").is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// #3997 — the superseding kill: `unreachable += nf()` where `nf` itself
/// is `Never`-typed exits at the call — the compound target's revive must
/// not erase the callee's kill, and the dead row stands (the official's
/// first dead node is the statement's own tail, `; print('after');`).
#[test]
fn compound_write_rhs_exit_keeps_the_kill() {
    let source = r#"Never get unreachable => throw 0;
Never nf() => throw 0;
void main() { unreachable += nf(); print('after'); }
"#;
    let report = report_ops_bare(source);
    assert_eq!(
        code_rows(&report, "assignment_to_final").len(),
        1,
        "{:#?}",
        rows(&report)
    );
    let dead = code_rows(&report, "dead_code");
    assert_eq!(dead.len(), 1, "{:#?}", rows(&report));
}

/// #3996 — the postfix twin: `unreachable++` assigns its operand, so the
/// write face reports; upstream exits at the postfix node and the trailing
/// `; print('after');` dead-codes beside `receiver_of_type_never`.
#[test]
fn increment_of_never_getter_reports_write_face_and_dead_tail() {
    let source = r#"Never get unreachable => throw 0;
void main() { unreachable++; print('after'); }
"#;
    let report = report_ops_bare(source);
    let hits = code_rows(&report, "assignment_to_final");
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(hits[0].range.length, 11, "{:#?}", rows(&report));
    let dead = code_rows(&report, "dead_code");
    assert_eq!(dead.len(), 1, "{:#?}", rows(&report));
}

/// #3996 — `unreachable ??= 3` reports the write face; the if-null
/// machinery reads the LHS upstream (`getType` → `isBottom` → exit), so
/// `dead_code` and `dead_null_aware_expression` both stand — the #3991
/// pin's face verbatim.
#[test]
fn if_null_write_to_never_getter_reports_write_face_and_dead_rows() {
    let source = r#"Never get unreachable => throw 0;
void main() { unreachable ??= 3; print('after'); }
"#;
    let report = report_ops_bare(source);
    assert_eq!(
        code_rows(&report, "assignment_to_final").len(),
        1,
        "{:#?}",
        rows(&report)
    );
    assert!(
        code_names(&report).contains(&"dead_code"),
        "{:#?}",
        rows(&report)
    );
    assert!(
        code_names(&report).contains(&"dead_null_aware_expression"),
        "{:#?}",
        rows(&report)
    );
}
