//! #3954 pins — the `!` staging gate's unknown-operand admission
//! (`Engine::check_null_assert`, engine_core.rs): the gate no longer refuses
//! an unknown-typed bare local read, so an implicitly typed formal and an
//! unannotated `var` reach the teardown promotion re-decision
//! (`flush_null_aware_staged`) exactly like `dynamic`. Upstream
//! `_checkForUnnecessaryNullAware` asks only `isStrictlyNonNullable` over the
//! promoted face — it never rejects an unknown declared type.
//!
//! The acceptance pair (`implicit_formal_promoted_bang_reports`,
//! `implicit_var_promoted_bang_reports`) lives in ticket_3933_tests.rs —
//! this file pins the bounds the relaxed gate must hold:
//!
//! | fixture                                    | oracle  |
//! | ------------------------------------------ | ------- |
//! | `void h(a) { a!; }`                        | silent  |
//! | `void k() { var a; a!; }`                  | silent  |
//! | `void m(dynamic a) { a!; }`                | silent  |
//! | `class C { Object? m; } … c.m!;`           | silent  |
//! | `var a; if (a is int) { a!; }`             | reports |
//!
//! An unknown-typed read that was never promoted stages but the join answers
//! `None` — the suppression lives in the promotion table, so dropping the
//! `ty != Ty::Unknown` conjunct cannot fabricate a report. The qualified
//! (`c.m!`) arm stays out through the unchanged `!qualified` conjunct.

use super::test_rig::report_ops;
use crate::FileReport;

/// The `code` rows of one report as `(offset, length)`, in source order.
fn spans_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// Byte offset of the `!` that CLOSES the first `ident!` in the fixture,
/// shifted past the core prelude the rig prepends.
fn bang_at(source: &str, ident: &str) -> u32 {
    let needle = format!("{ident}!");
    (super::test_rig::CORE_PRELUDE.len()
        + source.find(needle.as_str()).expect("needle in fixture")
        + ident.len()) as u32
}

// —— The silent bound: an unknown-typed read that never promoted ————————

/// An implicitly typed formal whose `!` sees no promotion keeps the
/// `dynamic` face — potentially nullable, so the assertion is live. The
/// relaxed gate stages the read, the promotion table answers `None`, and
/// nothing reports (oracle: silent).
#[test]
fn implicit_formal_unpromoted_bang_stays_silent() {
    let source = "\
void h(a) {
  a!;
}
";
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "an unpromoted implicit formal keeps the `!` live; got: {:#?}",
        report.diagnostics
    );
}

/// The `var` twin: `var a` binds `Ty::UNKNOWN`, and without a promotion
/// the `!` stays necessary (oracle: silent — only the unused-local row
/// fires, which this pin does not count).
#[test]
fn implicit_var_unpromoted_bang_stays_silent() {
    let source = "\
void k() {
  var a;
  a!;
}
";
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "an unpromoted `var` keeps the `!` live; got: {:#?}",
        report.diagnostics
    );
}

/// The `dynamic` bound the gate already held: no promotion, no report.
#[test]
fn dynamic_formal_unpromoted_bang_stays_silent() {
    let source = "\
void m(dynamic a) {
  a!;
}
";
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "an unpromoted `dynamic` keeps the `!` live; got: {:#?}",
        report.diagnostics
    );
}

// —— The qualified bound: `c.m!` never stages through the local gate ————

/// A member read's `!` stays out of the local staging arm — the unchanged
/// `!qualified` conjunct — so a nullable member keeps its assertion live
/// even where a same-named local is promotable (#2338's shadow guard).
/// Oracle: silent.
#[test]
fn qualified_nullable_member_bang_stays_silent() {
    let source = "\
class C {
  Object? m;
}

void q(C c) {
  c.m!;
}
";
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "a nullable member read keeps the `!` live; got: {:#?}",
        report.diagnostics
    );
}

// —— The positive face: unknown-typed `var` promoted by `is int` ————————

/// `var a` promoted through `is int` — a non-`Object` target, so the
/// promotion lands on a named type — reports the dead `!` at the operator,
/// the same shape the acceptance pair carries for `is Object`.
#[test]
fn implicit_var_int_promoted_bang_reports() {
    let source = "\
void r() {
  var a;
  if (a is int) {
    a!;
  }
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at(source, "a"), 1)],
        "the promoted `var`'s `!` is dead; got: {:#?}",
        report.diagnostics
    );
}
