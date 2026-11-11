//! #3933 pins — the two promotion staged-re-decision faces routed out of
//! the #3909 packet, plus the negative-evidence rows that bound them.
//!
//! Both faces were measured against the oracle on the current tree. The
//! diagnosis below is what the probes say, not what the routing guessed;
//! both roots turned out to live outside this ticket's write scope, so
//! this file carries the pin set and the routing evidence.
//!
//! ## Item 1 — the promoted `!` on an IMPLICITLY typed operand
//!
//! The divergence is **implicit typing vs a written annotation**, not
//! parameter vs local. Measured against the oracle (its row is
//! `WARNING|STATIC_WARNING|UNNECESSARY_NON_NULL_ASSERTION` at the `!`):
//!
//! | fixture                                       | oracle  | dartq  |
//! | --------------------------------------------- | ------- | ------ |
//! | `void f(dynamic a) { if (a is Object) a!; }`   | reports | reports |
//! | `void f(Object? a) { if (a is Object) a!; }`   | reports | reports |
//! | `void f(a)         { if (a is Object) a!; }`   | reports | SILENT  |
//! | `void f() { var a; if (a is Object) a!; }`     | reports | SILENT  |
//!
//! Both silent rows bind `Ty::UNKNOWN` — the suppress-everything sentinel:
//! an implicitly typed formal through `bind_parameter_type`'s
//! `unwrap_or(Ty::UNKNOWN)`, an unannotated `var` through
//! `bind_declared_type`'s `pending_decl_ty.unwrap_or(Ty::UNKNOWN)`. The
//! `!`-re-decision staging gate in `Engine::check_null_assert` refuses
//! every unknown-typed operand (`ty != Ty::Unknown && !qualified &&
//! self.local_hits.contains_key(...)`), so those reads never reach
//! [`crate::diagnose::Engine::flush_null_aware_staged`]'s re-decision
//! loop. The sibling null-aware faces do NOT carry that gate — they stage
//! on `local_hits` alone — which is why `a?.hashCode` and `a ?? 1` report
//! correctly over the very same implicit operand. The fix therefore
//! belongs to the staging gate in `engine_core.rs` (a live lane's file),
//! not to the flush in `unused_finalization.rs`.
//!
//! ## Item 2 — the `??=` RHS promotion leak
//!
//! The over-report is **not** cascade-section persistence. A plain `=`
//! RHS promotion legitimately reaches the next section (the oracle reports
//! the second `i!` of `c..x = i!..f(i!)`), and so does a bare cascade
//! (`c..f(i!)..f(i!)`). The `??=` shape is the odd one out, and its
//! minimal repro carries no cascade at all:
//!
//! ```text
//! void g(int? i) { int? y; y ??= i!; i!; }   // oracle: silent; dartq: reports
//! ```
//!
//! Upstream splits at `ifNullExpression_rightBegin` (the null path) and
//! rejoins at `ifNullExpression_end` with `_join(_current,
//! context._shortcutState)`, so a promotion the RHS made can never escape
//! the if-null expression. Our flattened replay runs the RHS inline and
//! restores the skip path only when the RHS left the state unreachable
//! (`if *if_null && … && !self.reachable()`), so a reachable RHS's
//! post-state — including the `i!` promotion — survives into the enclosing
//! flow. That join lives in the flow crate's assignment fold (a live
//! lane's file).

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

/// Byte offset of the FIRST `needle` in the fixture, shifted past the core
/// prelude the rig prepends.
fn at(source: &str, needle: &str) -> u32 {
    (super::test_rig::CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// Byte offset of the LAST `needle` in the fixture, shifted past the core
/// prelude — the cascade fixtures carry one `i!` per section.
fn at_last(source: &str, needle: &str) -> u32 {
    (super::test_rig::CORE_PRELUDE.len() + source.rfind(needle).expect("needle in fixture")) as u32
}

/// Byte offset of the `!` that CLOSES the first `ident!` in the fixture,
/// shifted past the core prelude — `unnecessary_non_null_assertion`
/// anchors on the operator, never on the operand.
fn bang_at(source: &str, ident: &str) -> u32 {
    let needle = format!("{ident}!");
    at(source, &needle) + ident.len() as u32
}

/// Byte offset of the `!` that closes the LAST `ident!` in the fixture.
fn bang_at_last(source: &str, ident: &str) -> u32 {
    let needle = format!("{ident}!");
    at_last(source, &needle) + ident.len() as u32
}

// —— Item 1: the promoted `!` — the written-annotation bounds ——————————

/// A written `dynamic` formal promotes through `is Object` and the `!` is
/// dead. This is the bound a fix must not disturb.
#[test]
fn dynamic_formal_promoted_bang_reports() {
    let source = "\
void f(dynamic a) {
  if (a is Object) {
    a!;
  }
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at(source, "a"), 1)],
        "{:#?}",
        report.diagnostics
    );
}

/// A written nullable formal promotes through `is Object` — the second
/// active bound. (`int?` is not usable here: the rig's stub `Object` is
/// not the real root type, so `int is Object` never narrows. `Object?` is
/// the stub-safe spelling of the same face.)
#[test]
fn nullable_formal_promoted_bang_reports() {
    let source = "\
void f(Object? a) {
  if (a is Object) {
    a!;
  }
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at(source, "a"), 1)],
        "{:#?}",
        report.diagnostics
    );
}

// —— Item 1: the promoted `!` — the implicit-typing residuals ————————

/// An implicitly typed formal's `!` after `is Object` is dead (oracle:
/// `WARNING|STATIC_WARNING|UNNECESSARY_NON_NULL_ASSERTION` at the `!`).
///
/// #3954 landed the staging-gate fix: `Engine::check_null_assert` no longer
/// refuses an unknown-typed operand, so the promotion re-decision runs for
/// the implicit formal exactly as it does for `dynamic`.
#[test]
fn implicit_formal_promoted_bang_reports() {
    let source = "\
void f(a) {
  if (a is Object) {
    a!;
  }
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at(source, "a"), 1)],
        "an implicitly typed formal promotes like `dynamic`; got: {:#?}",
        report.diagnostics
    );
}

/// The `var` twin of the same face: an unannotated `var a` binds
/// `Ty::UNKNOWN` too, and #3954's gate fix lets its promoted `!` reach the
/// re-decision. The two pins flip together — the gate, not the declaration
/// form, was the divergence.
#[test]
fn implicit_var_promoted_bang_reports() {
    let source = "\
void f() {
  var a;
  if (a is Object) {
    a!;
  }
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at(source, "a"), 1)],
        "an unannotated `var` promotes like `dynamic`; got: {:#?}",
        report.diagnostics
    );
}

// —— Item 2: the `??=` RHS must not leak its promotions ————————————————
//
// The three ACTIVE pins below are the negative evidence: the promotion
// DOES cross cascade-section boundaries for a plain `=` and for a bare
// cascade section list, exactly as the oracle says. A `??=` fix must not
// narrow those.

/// A plain `=` RHS promotion survives into the following cascade section:
/// `..x = i!` promotes `i`, so the next section's `i!` is dead. Oracle
/// confirms — this is behavior to preserve, not a bug.
#[test]
fn plain_write_rhs_promotion_reaches_the_next_cascade_section() {
    let source = "\
abstract class C {
  int? x;
  void f(int i);
}

void g(C c, int? i) {
  c
    ..x = i!
    ..f(i!);
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at_last(source, "i"), 1)],
        "only the second `i!` is dead; got: {:#?}",
        report.diagnostics
    );
}

/// A cascade section list without any `??=` shares the promotion across
/// sections — the second `i!` reports. The oracle agrees.
#[test]
fn cascade_sections_share_the_rhs_promotion() {
    let source = "\
abstract class C {
  void f(int i);
}

void g(C c, int? i) {
  c
    ..f(i!)
    ..f(i!);
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at_last(source, "i"), 1)],
        "the second section's `i!` is dead; got: {:#?}",
        report.diagnostics
    );
}

/// The same promotion outside any cascade: a plain `y = i!` promotes `i`
/// for the rest of the body, so the trailing `i!` is dead. Oracle: two
/// rows (the unused `y` beside the dead assertion).
#[test]
fn plain_write_rhs_promotion_reaches_the_enclosing_flow() {
    let source = "\
void g(int? i) {
  int? y;
  y = i!;
  i!;
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at_last(source, "i"), 1)],
        "only the trailing `i!` is dead; got: {:#?}",
        report.diagnostics
    );
}

/// `x ??= i!` runs its RHS on the null path alone, and upstream rejoins
/// that path with the shortcut state at `ifNullExpression_end`
/// (`_join(_current, context._shortcutState)`), so the RHS's promotion
/// never reaches the enclosing flow: the trailing `i!` is necessary and
/// nothing reports. Oracle: silent.
#[test]
fn if_null_write_rhs_does_not_promote_the_local() {
    let source = "\
void g(int? i) {
  int? y;
  y ??= i!;
  i!;
}
";
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "the `??=` RHS promotion must not survive the join; got: {:#?}",
        report.diagnostics
    );
}

/// The cascade shape of the same leak, one line: `c..x ??= i!..f(i!)`
/// reports on NEITHER `!` for the oracle — the first is necessary (the
/// receiver is still nullable at the RHS) and the second is necessary
/// because the `??=` RHS's promotion died at the join.
#[test]
fn cascade_if_null_write_rhs_does_not_leak_promotion() {
    let source = "\
abstract class C {
  int? x;
  void f(int i);
}

void g(C c, int? i) {
  c..x ??= i!..f(i!);
}
";
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "neither `!` is dead under `??=`; got: {:#?}",
        report.diagnostics
    );
}

/// The multi-line cascade twin, so a fix that special-cases the one-line
/// spelling is caught.
#[test]
fn cascade_if_null_write_rhs_multiline_does_not_leak_promotion() {
    let source = "\
abstract class C {
  int? x;
  void f(int i);
}

void g(C c, int? i) {
  c
    ..x ??= i!
    ..f(i!);
}
";
    let report = report_ops(source);
    assert!(
        spans_of(&report, "unnecessary_non_null_assertion").is_empty(),
        "neither `!` is dead under `??=`; got: {:#?}",
        report.diagnostics
    );
}

/// The within-RHS face: `y ??= i! + i!` promotes `i` for the SECOND
/// operand inside the same RHS (the null path is live there), so exactly
/// one row reports — the oracle's shape.
#[test]
fn if_null_write_rhs_promotes_within_the_rhs() {
    let source = "\
void g(int? i) {
  int? y;
  y ??= i! + i!;
}
";
    let report = report_ops(source);
    assert_eq!(
        spans_of(&report, "unnecessary_non_null_assertion"),
        vec![(bang_at_last(source, "i"), 1)],
        "the RHS-internal promotion stays; got: {:#?}",
        report.diagnostics
    );
}
