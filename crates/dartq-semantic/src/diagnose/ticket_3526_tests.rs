//! #3526 pins — the cross-pool `ArgListId` panic behind the `?.`, `!`, and
//! `== null` gates over a `FutureOr`-typed operand.
//!
//! # The defect
//!
//! Three entry arms ask `Engine::strictly_non_nullable` whether a
//! `FutureOr`-headed operand is ever null: the `?.`/`?..` receiver gate
//! (`invalid_null_aware_operator`), the `!` postfix gate
//! (`unnecessary_non_null_assertion`), and the `==`/`!=` null-comparison
//! gate (`unnecessary_null_comparison_*`). Each funnels through
//! `FileElements::is_strictly_non_nullable_in`, whose `FutureOr` arm reads
//! the operand's argument list through a handed [`ArgPool`]. The slots a
//! `FutureOr<A?>`/`FutureOr<Function>` local's declared type carries are
//! minted in the ENGINE's diagnosis pool — a caller that reached the unit
//! model's pool instead indexed another universe (`len is 1 but the index
//! is 1`, rc=4 on user input: the census's eight nnbd static_errors cases,
//! `null_assertion_ambiguous_test`, and `equality_static_test`).
//!
//! # The contract
//!
//! #3531 threaded the owning pool through the call so the arm resolves
//! argument lists in the universe that minted them. #3526 adds the
//! belt-and-suspenders arm: `strictly_non_nullable_within` resolves the
//! `FutureOr` argument through [`ArgPool::get_checked`], so a slot the
//! handed pool never interned — a `Ty` that crossed a pool boundary
//! unprojected — answers the conservative `false` (under-report, the
//! posture the recursion-depth arm already takes) instead of panicking.
//!
//! # The repros
//!
//! ```dart
//! class A { void test() {} }
//! void main() {
//!   FutureOr<A?> a = null;
//!   a?.test();   // ?. receiver gate over an engine-minted [A?] list
//!   FutureOr<Function> f = () {};
//!   f!;          // null-assert gate
//!   FutureOr<A?> c = null;
//!   c == null;   // null-comparison gate
//! }
//! ```
//!
//! Every arm must complete without a panic; the verdict each gate lands
//! is its own lane's business — these pins own only "does not crash" and
//! the arm's conservative `false` on a foreign slot.
//!
//! [`ArgPool`]: crate::model::ArgPool
//! [`ArgPool::get_checked`]: crate::model::ArgPool::get_checked

use super::test_rig::{CORE_PRELUDE, report_ops};

/// Every `(code, offset, len)` diagnostic `fixture` produces — the
/// arms' outcomes pooled into one shape so each pin asserts its own
/// gate's rows.
fn rows(fixture: &str) -> Vec<(&'static str, u32, u32)> {
    report_ops(fixture)
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// Offset of `needle` inside the first occurrence of `line`.
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source
        .find(line)
        .unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

/// The `?.` receiver arm (nnbd `static_errors_A14`/`A32`/`A33` family's
/// `a?.test()` shape): `FutureOr<A?>` is nullable-possible, so the
/// operator stays live and the pass completes — panic-free is the first
/// assertion, the live `?.` (no `invalid_null_aware_operator` row) the
/// second.
#[test]
fn null_aware_member_on_nullable_future_or_does_not_panic() {
    let fixture = "class A { void test() {} }\n\
                   void main() {\n\
                   \x20 FutureOr<A?> a = null;\n\
                   \x20 a?.test();\n\
                   }\n";
    let diagnostics = rows(fixture);
    assert!(
        diagnostics
            .iter()
            .all(|(code, ..)| *code != "invalid_null_aware_operator"),
        "a nullable-possible `FutureOr<A?>` keeps `?.` live: {diagnostics:?}"
    );
}

/// The `!` arm (`null_assertion_ambiguous_test`'s shape): `FutureOr`
/// alone reads `FutureOr<dynamic>` — nullable-possible, so the
/// assertion stays live and the pass completes.
#[test]
fn null_assert_on_bare_future_or_does_not_panic() {
    let fixture = "void main() {\n\
                   \x20 FutureOr f = null;\n\
                   \x20 f!;\n\
                   }\n";
    let diagnostics = rows(fixture);
    assert!(
        diagnostics
            .iter()
            .all(|(code, ..)| *code != "unnecessary_non_null_assertion"),
        "a bare `FutureOr` keeps `!` live: {diagnostics:?}"
    );
}

/// The `==` arm (`equality_static_test`'s `null == futureOrVar` shape):
/// `FutureOr<Covar1>` over a non-nullable argument is strictly
/// non-nullable at the argument-aware arm — the comparison is
/// constant-false where the plain shape face stays silent; either way
/// the pass must complete. The verdict pin is the crash-freedom plus
/// the arm's answer not flipping the oracle's posture.
#[test]
fn null_comparison_on_future_or_does_not_panic() {
    let l1 = "class Covar1 {}\n";
    let l2 = "void main() {\n";
    let l3 = "  Covar1 covar1Var = Covar1();\n";
    let l4 = "  FutureOr<Covar1> futureOrCovar1Var = covar1Var;\n";
    let l5 = "  null == futureOrCovar1Var;\n";
    let l6 = "}\n";
    let fixture = format!("{l1}{l2}{l3}{l4}{l5}{l6}");
    let diagnostics = rows(&fixture);
    // The `FutureOr<Covar1>` argument is strictly non-nullable, so the
    // `==` reads constant-false — the oracle's
    // `unnecessary_null_comparison` family lands at the comparison;
    // whichever member the arm emits, the pass may not crash and may
    // not stay silent-where-it-should-report on the strictly face.
    let comparison_rows: Vec<_> = diagnostics
        .iter()
        .filter(|(code, ..)| code.starts_with("unnecessary_null_comparison"))
        .collect();
    assert!(
        !comparison_rows.is_empty(),
        "a strictly-non-nullable `FutureOr<Covar1>` makes `== null` dead: {diagnostics:?}"
    );
    // The verdict anchors on the `null ==` line (the operator's own span
    // — wherever the arm lands it stays inside the statement's extent).
    let source = format!("{CORE_PRELUDE}{fixture}");
    let (start, end) = (at(&source, l5, "null"), at(&source, l5, ";"));
    assert!(
        comparison_rows
            .iter()
            .all(|(_, offset, _)| start <= *offset && *offset <= end),
        "the verdict anchors inside the comparison statement: {comparison_rows:?}"
    );
}
