//! The #3599 pins: the `unnecessary_type_check` verdict on a tear-off
//! operand interned with its declared required/optional split.
//!
//! The `FnShape` identity carries `required_positional` — the
//! `_isFunctionSubtypeOf` arity gate (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/subtype.dart:107-113) reads it to
//! tell `Function(int, [List])` from `Function(int, List)`. Every
//! tear-off face the verdict's operand side builds (`member_entry_
//! tear_off_ty`, `ctor_tear_off_ty`, `tearoff_ty`, `tear_off_fn_ty`)
//! interned through the all-required producers instead, so a method
//! declared `m(int, [List])` read as a value carried the shape
//! `Function(int, List)` — a decided miss (`from.required = 2 >
//! to.required = 1`) where the oracle's tear-off `staticType` proves
//! the check constant-true. The census's second-largest code family
//! (the function_type corpus's `mN is Function(...)` rows and the
//! co19 Constructor-tear-offs `C.new`/`C.id` checks) sat silent on
//! exactly this leg.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The exact `(offset, length)` rows of one code, source-ordered.
fn exact_rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The fixture-relative anchor shifted by the rig's prepended prelude.
fn anchor(fixture: &str, needle: &str) -> u32 {
    CORE_PRELUDE.len() as u32 + fixture.find(needle).map(|i| i as u32).unwrap()
}

/// An instance method with an optional positional formal carries its
/// declared split into the tear-off face: `m3 is Function(int,
/// [List<Function>])` is constant-true upstream (the oracle's row spans
/// the whole `is` expression, 'true' polarity). Before the fix the
/// operand interned `Function(int, List<Function>)` and the arity gate
/// abstained.
#[test]
fn method_tear_off_with_optional_positional_reports_constant_true() {
    let fixture = "class U {\n  m3(int x, [List<Function> x0 = const []]) => throw 'x';\n  test() {\n    m3 is Function(int x, [List<Function>]);\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(fixture, "m3 is Function"), 39)],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].message,
        "Unnecessary type check; the result is always 'true'."
    );
}

/// The same split on a `this`-receiver member read: `this.m3 is
/// Function(int, [List<Function>])` answers through the member walk
/// (`member_tear_off_ty` shares `member_entry_tear_off_ty`).
#[test]
fn this_member_tear_off_reports_constant_true() {
    let fixture = "class U {\n  m3(int x, [List<Function> x0 = const []]) => throw 'x';\n  test() {\n    this.m3 is Function(int x, [List<Function>]);\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(fixture, "this.m3 is Function"), 44)],
        "{:?}",
        report.diagnostics
    );
}

/// A constructor selector with an optional formal: `C.new is
/// C Function([int])` is constant-true — `ctor_tear_off_ty` interned
/// `C Function(int)` before the fix, which `is C Function([int])` could
/// not prove.
#[test]
fn ctor_tear_off_with_optional_positional_reports_constant_true() {
    let fixture = "class C {\n  C([int x = 0]);\n}\nmain() {\n  C.new is C Function([int]);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(fixture, "C.new is C Function"), 26)],
        "{:?}",
        report.diagnostics
    );
}

/// The alias-typedef target of the function_type corpus: `m3 is
/// F3<int>` over `typedef F3<T> = Function(int, [List<Function>])` —
/// the checked side folds through the parameterized-alias arm, the
/// operand keeps its declared split, both answer the strict function
/// rule true.
#[test]
fn method_tear_off_against_alias_target_reports_constant_true() {
    let fixture = "typedef F3<T> = Function(int x, [List<Function>]);\nclass U {\n  m3(int x, [List<Function> x0 = const []]) => throw 'x';\n  test() {\n    m3 is F3<int>;\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(fixture, "m3 is F3<int>"), 13)],
        "{:?}",
        report.diagnostics
    );
}

/// A target that accepts strictly fewer required calls still proves
/// the check: `m3(int, [List])` accepts every two-argument call
/// `Function(int, List)` spells, so the oracle reports constant-true
/// (the arity gate admits `from.required <= to.required`).
#[test]
fn wider_acceptor_against_all_required_target_reports_constant_true() {
    let fixture = "class U {\n  m3(int x, [List<Function> x0 = const []]) => throw 'x';\n  test() {\n    m3 is Function(int x, List<Function>);\n  }\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        exact_rows(&report, "unnecessary_type_check"),
        vec![(anchor(fixture, "m3 is Function"), 37)],
        "{:?}",
        report.diagnostics
    );
}

/// Control: a target demanding more positionals than the method
/// declares is informative — `m3 is Function(int, List<Function>, int)`
/// can never hold (the oracle emits no row) and must not fabricate.
#[test]
fn arity_beyond_the_declared_group_stays_silent() {
    let fixture = "class U {\n  m3(int x, [List<Function> x0 = const []]) => throw 'x';\n  test() {\n    m3 is Function(int x, List<Function>, int);\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        exact_rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// Control: a mismatched formal is informative — `m3 is Function(
/// String, [List<Function>])` can hold at runtime (the oracle emits no
/// row); the verdict must not fabricate.
#[test]
fn mismatched_formal_stays_silent() {
    let fixture = "class U {\n  m3(int x, [List<Function> x0 = const []]) => throw 'x';\n  test() {\n    m3 is Function(String x, [List<Function>]);\n  }\n}\n";
    let report = report_ops(fixture);
    assert!(
        exact_rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}
