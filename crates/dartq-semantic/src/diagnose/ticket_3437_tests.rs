//! The #3437 pins: `a[i] ??= v` stages `dead_null_aware_expression` through
//! the index-result row's read face — upstream `setReadElement`'s
//! `IndexExpression` arm answers `operator[]`'s return type, so the `??=`
//! target read is the folded `[]` send, never the receiver row. The row
//! model folds an index access into the receiver's `Intent::Operator`
//! row plus a `RowShape::Value` result stand-in at the `]` (the
//! `index_result_recvs` latch); the target scan's named-row gate rejects
//! both shapes, so the read the oracle takes through `x[index]` never
//! reached the coalesce verdict.
//!
//! Oracle provenance: `dart analyze` 3.13.1 — `null_aware_operator_A13_t13`
//! reports the dead value for `x?[index] ??= value` (the desugared
//! `SHORT[EXP(x), fn[t] => t[index] ??= value]` reads `t[index]`'s
//! non-nullable `[]` return) and for `n0[index] ??= value` on the
//! promoted local; the same verdict covers the plain `a[i] ??= v` arm.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// `(offset, length)` of `needle`'s LAST occurrence inside the FIXTURE
/// text, shifted past the prelude the rig mounts ahead of it — the
/// last-occurrence read keys the USE site when the fixture's declaration
/// carries the same spelling (`value`'s `[]=` formal vs its `??=` read).
fn at(source: &str, needle: &str) -> (u32, u32) {
    let found = source
        .rfind(needle)
        .unwrap_or_else(|| panic!("`{needle}` missing"));
    (
        found as u32 + CORE_PRELUDE.len() as u32,
        needle.len() as u32,
    )
}

/// The `dead_null_aware_expression` rows as `(offset, length)`,
/// source-ordered.
fn dead_rows(source: &str) -> Vec<(u32, u32)> {
    let report = report_ops(source);
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "dead_null_aware_expression")
        .map(|row| (row.range.offset, row.range.length))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// `a[i] ??= v` over a `[]` whose return is non-nullable: the folded
/// result stand-in reads `int`, so the value is dead (the A13
/// `n0[index] ??= value` arm).
#[test]
fn if_null_assignment_over_an_index_reports_the_dead_value() {
    let source = "\
class C {\n\
\x20 int operator [](int index) => index;\n\
\x20 void operator []=(int index, int value) {}\n\
}\n\
void f(C n0, int index, int value) {\n\
\x20 n0[index] ??= value;\n\
}\n";
    assert_eq!(dead_rows(source), vec![at(source, "value")]);
}

/// The `?[` twin desugars to `t[index] ??= value` over the non-null temp
/// (`SHORT[EXP(x), fn[t] => t[index] ??= value]`): the `?[` null-aware
/// join never reaches the `??=` read, so the dead value reports on a
/// nullable receiver the same way.
#[test]
fn if_null_assignment_over_a_null_aware_index_reports_the_dead_value() {
    let source = "\
class C {\n\
\x20 int operator [](int index) => index;\n\
\x20 void operator []=(int index, int value) {}\n\
}\n\
void f(C? x, int index, int value) {\n\
\x20 var actual = x?[index] ??= value;\n\
}\n";
    assert_eq!(dead_rows(source), vec![at(source, "value")]);
}

/// A nullable `[]` return keeps the right operand live: `V? operator []`
/// (the `Map` shape) answers a nullable read, so `??=` is no dead write.
#[test]
fn if_null_assignment_over_a_nullable_index_result_stays_silent() {
    let source = "\
class C {\n\
\x20 int? operator [](int index) => null;\n\
\x20 void operator []=(int index, int value) {}\n\
}\n\
void f(C c, int index, int value) {\n\
\x20 c[index] ??= value;\n\
}\n";
    assert_eq!(dead_rows(source), Vec::new());
}

/// The plain-compound control: `a[i] += v` over a non-nullable `[]` is an
/// ordinary compound write — no dead-coalesce face exists for `+=`, and
/// the index target must not grow one through the new arm.
#[test]
fn compound_assignment_over_an_index_stays_silent() {
    let source = "\
class C {\n\
\x20 int operator [](int index) => index;\n\
\x20 void operator []=(int index, int value) {}\n\
}\n\
void f(C c, int index, int value) {\n\
\x20 c[index] += value;\n\
}\n";
    assert_eq!(dead_rows(source), Vec::new());
}
