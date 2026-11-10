//! #3802 pins — the index face's `super` receiver.
//!
//! Upstream `_checkForUnnecessaryNullAware` bails on a `super` target
//! before the type test (`if (target is SuperExpression) return;`,
//! upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_verifier.dart:7301). No
//! `super?[i]` spelling reaches an `IndexExpression`: the parser rejects
//! the selector (`super![0]` is the only legal spelling, and `super![i]?[j]`
//! is not a receiver the check sees), so the oracle emits no
//! `invalid_null_aware_operator` row on a `super` receiver while
//! `check_null_assert`'s `non_null_form` rewrite makes the row read
//! strictly non-nullable.
//!
//! The listener's index face decides from that rewritten row
//! (`handle_indexed_expression`), so the pins below hold both directions:
//! a `super` receiver stays silent, and the ordinary `?[`/`?..[` faces —
//! which share the emit — keep reporting on a non-nullable receiver and
//! stay silent on a nullable one.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `#[path]` keeps
//! the module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The absolute offset of the first occurrence of `needle` in the fixture
/// as the rig analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture.find(needle).expect("needle appears in the fixture");
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `invalid_null_aware_operator` messages, in report order.
fn messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `invalid_null_aware_operator` rows as `(offset, length)`, ascending.
fn anchors(report: &FileReport) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// `super!?[0]`: the `!` rewrites the receiver row to the non-nullable
/// container face, but the official check returns on the `super` target —
/// no row.
#[test]
fn super_receiver_index_is_silent() {
    let fixture = "\
class A {
  int operator [](int i) => 0;
}
class B extends A {
  void f() {
    super!?[0];
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        Vec::<String>::new(),
        "`super!?[0]` reports no null-aware row; got: {:?}",
        report.diagnostics
    );
}

/// The same guard must not silence the ordinary index face: a non-nullable
/// list receiver still reports at the `?[` token.
#[test]
fn non_nullable_index_receiver_reports() {
    let fixture = "\
void f(List<int> l) {
  l?[0];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![
            "The receiver can't be null, so the null-aware operator '?[' is unnecessary."
                .to_string()
        ],
        "`l?[0]` over a non-nullable list reports once; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "?["), 2)],
        "the anchor is the `?[` token"
    );
}

/// A nullable list receiver keeps the operator live: the warning is about a
/// receiver that cannot be null, never about a potentially-null one.
#[test]
fn nullable_index_receiver_is_silent() {
    let fixture = "\
void f(List<int>? n) {
  n?[0];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        Vec::<String>::new(),
        "`n?[0]` over a nullable list stays silent; got: {:?}",
        report.diagnostics
    );
}

/// The cascade index section (`?..[`) shares the emit: a non-nullable
/// target still reports at the whole `?..` operator.
#[test]
fn non_nullable_cascade_index_receiver_reports() {
    let fixture = "\
class C {
  int operator [](int i) => 0;
}
void f(C c) {
  c?..[0];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![
            "The receiver can't be null, so the null-aware operator '?..' is unnecessary."
                .to_string()
        ],
        "`c?..[0]` over a non-nullable target reports once; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "?.."), 3)],
        "the anchor is the `?..` token"
    );
}
