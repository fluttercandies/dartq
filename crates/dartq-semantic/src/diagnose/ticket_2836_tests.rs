//! The #2836 cast-initializer pins: a plain local whose initializer is an
//! `as`-cast expression (`var d = 1 as num;`) is judged on read accounting
//! exactly like every other local — unread reports `unused_local_variable`
//! at the name, read stays silent (upstream
//! unused_local_elements_verifier.dart's visitLocalVariableElement
//! :1259-1280 — the cast shape is not a verdict input).
//!
//! Verdict record: the census rows that motivated this ticket (28 UV
//! misses under the specify_nonobvious family, report.tsv 2026-10-29
//! 13:40) do NOT reproduce on the current tree — the sweep's binary
//! predates the wave that landed the for-in/pattern UV faces. These pins
//! lock the faces against regression; the family's residual misses of the
//! `specify_nonobvious_local_variable_types` code itself (for-in loop
//! variables, invocation type arguments) are the lints crate's face, not
//! the engine's.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `unused_local_variable` rows as `message @offset:len`, source
/// order — the exact-row pin format.
fn uv_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| format!("{} @{}:{}", d.message, d.range.offset, d.range.length))
        .collect()
}

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// `The value of the local variable '<name>' isn't used.` at `offset`.
fn uv(name: &str, offset: usize) -> String {
    format!(
        "The value of the local variable '{name}' isn't used. @{offset}:{}",
        name.len()
    )
}

/// The repro shape: a cast-initialized local unread reports the
/// `unused_local_variable` row at its name — the initializer's cast is
/// not a use and not a verdict input (the specify_nonobvious census
/// family's 28 rows all carry this shape).
#[test]
fn cast_initialized_local_unread_reports() {
    let fixture = "void f() {\n  var d = 1 as num;\n}\n";
    let report = report_ops(fixture);
    let d = at(fixture, "var d") + "var ".len();
    assert_eq!(uv_rows(&report), [uv("d", d)], "{:?}", uv_rows(&report));
}

/// The same cast-initialized shape read in the body stays silent.
#[test]
fn cast_initialized_local_read_stays_silent() {
    let fixture = "void f() {\n  var d = 1 as num;\n  if (d > 0) {}\n}\n";
    let report = report_ops(fixture);
    assert!(uv_rows(&report).is_empty(), "{:?}", uv_rows(&report));
}

/// The plain (non-cast) initializer baseline: an unread local reports —
/// the ordinary face these locks stand beside.
#[test]
fn plain_initialized_local_unread_still_reports() {
    let fixture = "void f() {\n  var d = 1;\n}\n";
    let report = report_ops(fixture);
    let d = at(fixture, "var d") + "var ".len();
    assert_eq!(uv_rows(&report), [uv("d", d)], "{:?}", uv_rows(&report));
}

/// The exact census case (SpecifyNonObviousLocalVariableTypesTest.test_as,
/// the family's materialized source): an untyped `f()` and the cast
/// initializer together — the row reports at `d`.
#[test]
fn census_test_as_case_reports() {
    let fixture = "f() {\n  var d = 1 as num;\n}\n";
    let report = report_ops(fixture);
    let d = at(fixture, "var d") + "var ".len();
    assert_eq!(uv_rows(&report), [uv("d", d)], "{:?}", uv_rows(&report));
}

/// The census `test_as_dynamic` case: a `dynamic` cast target keeps the
/// same read-accounting verdict.
#[test]
fn census_test_as_dynamic_case_reports() {
    let fixture = "f() {\n  var d = 1 as dynamic;\n}\n";
    let report = report_ops(fixture);
    let d = at(fixture, "var d") + "var ".len();
    assert_eq!(uv_rows(&report), [uv("d", d)], "{:?}", uv_rows(&report));
}

/// The census `test_cascade` case (whole-case source): a cascade
/// initializer is still no read — the unread `a` reports.
#[test]
fn census_test_cascade_case_reports() {
    let fixture = "f() {\n  var a = A()..x..x..x;\n}\n\nclass A {\n  final x = 0;\n}\n";
    let report = report_ops(fixture);
    let a = at(fixture, "var a") + "var ".len();
    assert_eq!(uv_rows(&report), [uv("a", a)], "{:?}", uv_rows(&report));
}
