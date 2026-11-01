//! #3436 — the callable-typed callee's return never reached the flow fold.
//!
//! The send close staged `send_result` into `send_return_staging`, but the
//! listener's `staged_call` arm pattern-matched `let Ty::Named` and dropped
//! every unit-kind return — `Null`, `dynamic`, `void`, `Never` — before
//! `stage_call_result` ran. A callable-typed callee (`Null Function() nf`)
//! reads `Base::Function` at name level, whose nullability the fold cannot
//! answer: the staged row is the invocation's only static-type source, so
//! `null == nf()` and `nf() is int` folded nothing and the dead arms the
//! oracle reports went missing.
//!
//! The arm now maps `Ty::Null → FlowBase::Null`, `Ty::Dynamic →
//! FlowBase::Dynamic`, `Ty::Void → FlowBase::Void`, `Ty::Never →
//! FlowBase::Never` — `FlowBase::Null` (never `Name("Null")`) is the only
//! face both type-ops backends read identically (`NameTypeOps::classify`
//! answers `NullOrEquivalent` on the identity; a `Name("Null")` walks as an
//! opaque class and folds `null == nf()` backwards). `Ty::Function`,
//! `Ty::Record` and the free-parameter `Unknown` still drop — under-report
//! stands ahead of a fabricated face.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The unit source as the rig analyzed it (the prelude prefix included, so
/// diagnostic offsets index this string).
fn unit_source(body: &str) -> String {
    format!("{CORE_PRELUDE}{body}")
}

/// The UTF-16 slice of `[offset, offset + length)` over `source`.
fn utf16_slice(source: &str, offset: u32, length: u32) -> String {
    let end = offset + length;
    let mut out = String::new();
    let mut at = 0u32;
    for ch in source.chars() {
        let width = ch.len_utf16() as u32;
        if at >= end {
            break;
        }
        if at >= offset {
            out.push(ch);
        }
        at += width;
    }
    out
}

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// The source text a row covers, over the prelude-prefixed unit.
fn row_text(source: &str, row: &(u32, u32, String)) -> String {
    utf16_slice(source, row.0, row.1)
}

/// A `Null`-returning callable-typed formal: `null == nf()` is the oracle's
/// constant-true comparison (the invoked result IS `Null`), so the `else`
/// arm dies and the `then` arm stays live. Without the staged `Null` face
/// the name-level fold read only `Base::Function` — signature-less — and
/// folded nothing, so the dead `else` never reported.
#[test]
fn null_returning_callable_call_result_reaches_the_flow_fold() {
    let body = "\
void f({required Null Function() nullFunction}) {
  if (null == nullFunction()) {
    int liveThen = 0;
  } else {
    int deadElse = 0;
  }
}
";
    let source = unit_source(body);
    let report = report_ops(body);
    let dead = rows_of(&report, "dead_code");
    assert_eq!(dead.len(), 1, "rows: {:#?}", report.diagnostics);
    let text = row_text(&source, &dead[0]);
    assert!(
        text.contains("deadElse"),
        "the else arm must be the dead one, got {text:?} — rows: {:#?}",
        report.diagnostics
    );
}

/// The `is` twin: `nf() is int` over a `Null`-returning callee is a
/// trivially unsatisfiable check (`Null` is never `int`), so the `then`
/// arm dies — the same staged `Null` face, read through the `is` test's
/// non-reference arm. `FlowBase::Null` (not `Name("Null")`) is what makes
/// both backends answer `NullOrEquivalent` here.
#[test]
fn null_returning_callable_is_check_deads_the_then_arm() {
    let body = "\
void f({required Null Function() nullFunction}) {
  if (nullFunction() is int) {
    int deadThen = 0;
  } else {
    int liveElse = 0;
  }
}
";
    let source = unit_source(body);
    let report = report_ops(body);
    let dead = rows_of(&report, "dead_code");
    assert_eq!(dead.len(), 1, "rows: {:#?}", report.diagnostics);
    let text = row_text(&source, &dead[0]);
    assert!(
        text.contains("deadThen"),
        "the then arm must be the dead one, got {text:?} — rows: {:#?}",
        report.diagnostics
    );
}

/// A non-nullable callable-typed formal keeps `??`'s guaranteed-non-null
/// face: `intFunction() ?? other` reads `int` (non-nullable) on the LHS,
/// so `other` is dead at 3.15 — the staged `int` face, same mechanism
/// #3415 pinned. The unit-kind extension must not regress it.
#[test]
fn non_nullable_callable_call_result_keeps_if_null_rhs_dead() {
    let body = "\
void f({required int Function() intFunction, required int other}) {
  intFunction() ?? other;
}
";
    let source = unit_source(body);
    let report = report_ops(body);
    let dead = rows_of(&report, "dead_code");
    assert_eq!(dead.len(), 1, "rows: {:#?}", report.diagnostics);
    let text = row_text(&source, &dead[0]);
    assert!(
        text.contains("other"),
        "the ?? RHS must be the dead one, got {text:?} — rows: {:#?}",
        report.diagnostics
    );
}

/// A nullable callable-typed formal (`int? Function()`) keeps `??`'s RHS
/// live: the staged `int?` face answers "possibly null" on the LHS, so
/// `other` is reachable — the under-report direction when the return's
/// nullability is genuinely undecidable.
#[test]
fn nullable_callable_call_result_keeps_if_null_rhs_live() {
    let body = "\
void f({required int? Function() nullableFunction, required int other}) {
  nullableFunction() ?? other;
}
";
    let report = report_ops(body);
    let dead = rows_of(&report, "dead_code");
    assert!(
        dead.is_empty(),
        "a nullable return must keep ??'s RHS live, rows: {:#?}",
        report.diagnostics
    );
}

/// A generic callable-typed formal (`T Function<T>(T)`) stages nothing:
/// the type-variable return is no spellable face, so the arm drops it and
/// neither fold fires — the under-report direction, exactly as #3415's
/// `undecidable_call_result_deads_nothing` pins.
#[test]
fn generic_callable_call_result_stages_nothing_fabricated() {
    let body = "\
void f<T>({required T Function<T>(T x) make}) {
  if (make(0) is int) {
    int liveThen = 0;
  } else {
    int liveElse = 0;
  }
}
";
    let report = report_ops(body);
    assert!(
        rows_of(&report, "dead_code").is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
    let checks: Vec<&'static str> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .filter(|name| name.starts_with("unnecessary_type_check"))
        .collect();
    assert!(checks.is_empty(), "rows: {:#?}", report.diagnostics);
}

/// The `dynamic`-returning callable (`dynamic Function() d`): `d() is int`
/// is a genuinely useful check — `dynamic` may hold any value, so the
/// oracle reports nothing on either arm (the check stays live, exactly
/// like a `dynamic` variable's `is`). Staging `FlowBase::Dynamic` keeps
/// that face; a fabricated `Function`/non-null base would fold the check
/// into a verdict the oracle never gives.
#[test]
fn dynamic_returning_callable_call_result_keeps_the_check_live() {
    let body = "\
void f({required dynamic Function() dyn}) {
  if (dyn() is int) {
    int liveThen = 0;
  } else {
    int liveElse = 0;
  }
}
";
    let report = report_ops(body);
    assert!(
        rows_of(&report, "dead_code").is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
    assert!(
        rows_of(&report, "unnecessary_type_check").is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}
