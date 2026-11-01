//! #3327 pins — the `?` collection sites' operand-type face.
//!
//! Upstream `_checkForUnnecessaryNullAware` gates on
//! `target.staticType` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/error_verifier.dart:7295-7408), and the
//! three collection call sites hand it the AST node's own expression:
//! `visitSpreadElement` passes `node.expression`, `visitNullAwareElement`
//! passes `node.value`, and `visitMapLiteralEntry` passes `node.key` /
//! `node.value`. The listener reads the row stack instead, and the row's
//! push-time payload is NOT that type in two directions:
//!
//! - a completed send's payload is the CALLEE's declared type (`...?f()`
//!   over `int? f()` read `int? Function()`, strictly non-nullable, and
//!   reported a live operator — the over-report arm);
//! - an untyped collection literal's row is an opaque `Value` (`...?[1, 2]`,
//!   `?[1, 2]`, `{?[1, 2]: v}` never reported — the under-report arm).
//!
//! The three sites now read `Engine::operand_static_ty` — the composite
//! static type (the send's result and its implicit-`call` dispatch, an
//! untyped literal's inferred `List`/`Set`/`Map`, the accessor face, the
//! assertion-rewritten row type). These pins hold that face per operator
//! kind plus the silent negatives.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `#[path]` keeps
//! the module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The absolute offset of the `nth` occurrence (0-based) of `needle` in the
/// fixture as the rig analyzes it: `report_ops` prefixes [`CORE_PRELUDE`],
/// so every fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str, nth: usize) -> u32 {
    let mut from = 0;
    for _ in 0..nth {
        from += fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture")
            + needle.len();
    }
    let hit = from + fixture[from..].find(needle).expect("needle appears in the fixture");
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

/// The `?...` message (the `{0}` operator and `{1}` replacement slots).
fn spread_message() -> String {
    "The receiver can't be null, so the null-aware operator '?...' is unnecessary.".to_string()
}

/// The element face's message (`_NullAwareKind.element`).
fn element_message() -> String {
    "The element can't be null, so the null-aware operator '?' is unnecessary.".to_string()
}

/// The map-entry key face's message (`_NullAwareKind.mapEntryKey`).
fn key_message() -> String {
    "The map entry key can't be null, so the null-aware operator '?' is unnecessary.".to_string()
}

// —— untyped collection-literal operands (the under-report arm) ————————

/// `[...?[1, 2]]`: the spread operand is an untyped list literal, whose
/// static type is the non-nullable `List<int>` inference — the `?...` is
/// dead and reports at the operator token.
#[test]
fn spread_over_untyped_list_literal_reports() {
    let fixture = "\
void f() {
  final a = [...?[1, 2]];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![spread_message()],
        "`...?[1, 2]` reports once; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "...?", 0), 4)],
        "the anchor is the `...?` token"
    );
}

/// The set and map literal spellings take the same face: `[...?{1, 2}]`
/// infers `Set<int>` and `{...?{'a': 1}}` infers `Map<String, int>`, both
/// strictly non-nullable.
#[test]
fn spread_over_untyped_set_and_map_literals_report() {
    let fixture = "\
void f() {
  final a = [...?{1, 2}];
  final b = {...?{'a': 1}};
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![
            (at(fixture, "...?", 0), 4),
            (at(fixture, "...?", 1), 4),
        ],
        "both literal spellings report at their own `...?`; got: {:?}",
        report.diagnostics
    );
}

/// The empty literal spellings keep the same non-nullable face: `[]` infers
/// the non-nullable `List`, `{}` the non-nullable `Map`.
#[test]
fn spread_over_empty_literals_reports() {
    let fixture = "\
void f() {
  final a = [...?[]];
  final b = [...?{}];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![spread_message(), spread_message()],
        "`...?[]` and `...?{{}}` report; got: {:?}",
        report.diagnostics
    );
}

// —— send operands (the over-report arm) ————————————————————————————————

/// `...?f()` over `int? f()`: the operand's static type is the call's
/// RESULT, so the `?...` is live — silent. The push-time payload is the
/// callee's own `int? Function()` face, which reported before this fix.
#[test]
fn spread_over_nullable_call_result_stays_silent() {
    let fixture = "\
int? g() => null;

void f() {
  final a = [...?g()];
}
";
    let report = report_ops(fixture);
    assert!(
        messages(&report).is_empty(),
        "a nullable call result keeps `...?` meaningful; got: {:?}",
        report.diagnostics
    );
}

/// The non-nullable twin: `int g()` makes the operand the non-nullable
/// `int` and the operator dead.
#[test]
fn spread_over_non_nullable_call_result_reports() {
    let fixture = "\
int g() => 1;

void f() {
  final a = [...?g()];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "...?", 0), 4)],
        "a non-nullable call result reports; got: {:?}",
        report.diagnostics
    );
}

/// A callable-class callee runs the implicit `call` dispatch: the send's
/// static type is `call`'s return, so an `int? call` operand stays live.
#[test]
fn spread_over_nullable_callable_class_result_stays_silent() {
    let fixture = "\
class C {
  int? call(String s) => null;
}

void f(C c) {
  final a = [...?c('x')];
}
";
    let report = report_ops(fixture);
    assert!(
        messages(&report).is_empty(),
        "a nullable `call` result keeps `...?` meaningful; got: {:?}",
        report.diagnostics
    );
}

/// Its non-nullable twin reports.
#[test]
fn spread_over_non_nullable_callable_class_result_reports() {
    let fixture = "\
class C {
  int call(String s) => 0;
}

void f(C c) {
  final a = [...?c('x')];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "...?", 0), 4)],
        "a non-nullable `call` result reports; got: {:?}",
        report.diagnostics
    );
}

/// `...?e!`: the assertion rewrites the read's static type to the
/// non-nullable form, so the operator is dead. The push-time payload keeps
/// the declared `int?`, which stayed silent before this fix.
#[test]
fn spread_over_asserted_read_reports() {
    let fixture = "\
void f(int? e) {
  final a = [...?e!];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "...?", 0), 4)],
        "`...?e!` reports; got: {:?}",
        report.diagnostics
    );
}

// —— element and map-entry operands ————————————————————————————————————

/// `?[1, 2]`: the element's value is an untyped list literal, so the `?` is
/// dead. The anchor is the `?` token alone (len 1).
#[test]
fn element_over_untyped_list_literal_reports() {
    let fixture = "\
void f() {
  final a = <List<int>>[?[1, 2]];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![element_message()],
        "`?[1, 2]` reports once; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "[?[", 0) + 1, 1)],
        "the anchor is the `?` token"
    );
}

/// `{?[1, 2]: v}`: the map entry's KEY is an untyped list literal, so the
/// key `?` is dead.
#[test]
fn map_entry_key_over_untyped_list_literal_reports() {
    let fixture = "\
void f() {
  final a = <List<int>, int>{?[1, 2]: 1};
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![key_message()],
        "`{{?[1, 2]: 1}}` reports at the key face; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "{?[", 0) + 1, 1)],
        "the anchor is the key's `?` token"
    );
}

/// The map-entry value face keeps both directions: a non-nullable literal
/// value reports, a declared nullable one stays silent.
#[test]
fn map_entry_value_face_reports_literals_and_spares_nullables() {
    let fixture = "\
void f(int? e) {
  final a = <String, int>{'a': ?1};
  final b = <String, int>{'a': ?e};
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![
            "The map entry value can't be null, so the null-aware operator '?' is unnecessary."
                .to_string()
        ],
        "only the literal value reports; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, ": ?1", 0) + 2, 1)],
        "the anchor is the value's `?` token"
    );
}

// —— trailing `??` composites (the join face) ————————————————————————

/// `??` pushes no result row, so the stack top is its RIGHT operand while
/// the expression types as the join (`int? ?? int` is `int`). The join is
/// not reachable from the listener: `stage_if_null_result` records it only
/// when the right operand leaves a row of its own (`rows.len() == mark + 1`
/// plus two `Ty::Named` faces), and a folded literal (`?? 1`) leaves none —
/// so the top row stays the LEFT operand's read. The expectation is the
/// oracle's: all three operator kinds report.
#[test]
#[ignore = "`??` join unreachable: the if-null staging needs an rhs-less (folded) variant"]
fn if_null_composite_operands_report_every_operator_kind() {
    let fixture = "\
void f(int? e) {
  final a = [...?e ?? 1];
  final b = <int>[?e ?? 1];
  final c = <int, int>{?e ?? 1: 2};
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![
            (at(fixture, "...?", 0), 4),
            (at(fixture, "[?e", 0) + 1, 1),
            (at(fixture, "{?e", 0) + 1, 1),
        ],
        "each operator kind reads the join; got: {:?}",
        report.diagnostics
    );
}

/// The nullable twin: `int? ?? int?` joins to a nullable head, so the
/// operator stays meaningful in every kind.
#[test]
fn if_null_composite_operands_stay_silent_when_the_join_is_nullable() {
    let fixture = "\
void f(int? e, int? d) {
  [...?e ?? d];
  <int?>[?e ?? d];
}
";
    let report = report_ops(fixture);
    assert!(
        messages(&report).is_empty(),
        "a nullable join keeps the operator live; got: {:?}",
        report.diagnostics
    );
}

// —— flow-promotion residuals (the flow-side channel) ——————————————————
//
// `_checkForUnnecessaryNullAware` reads the FLOW-augmented static type, so a
// read promoted by `if (e != null)` makes the operator dead. The listener
// cannot answer it: `Engine::read_types` fills only at the teardown join
// (`diagnose.rs`), and the deferred re-decision channel
// (`Engine::null_aware_staged`) carries only the `?.`/`?..`/`?[` kinds, its
// flush living outside this ticket's write scope. These two pins hold the
// expected verdicts so the flow-side fix lands against them.

/// `[...?e]` after `if (e != null)` — promoted to `int`, the operator is
/// dead (oracle reports).
#[test]
#[ignore = "flow promotion: needs a teardown re-decision kind in null_aware_staged"]
fn spread_over_promoted_read_reports() {
    let fixture = "\
void f(int? e) {
  if (e != null) {
    final a = [...?e];
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "...?", 0), 4)],
        "a promoted read makes `...?` dead; got: {:?}",
        report.diagnostics
    );
}

/// The element twin: `?e` after `if (e != null)`.
#[test]
#[ignore = "flow promotion: needs a teardown re-decision kind in null_aware_staged"]
fn element_over_promoted_read_reports() {
    let fixture = "\
void f(int? e) {
  if (e != null) {
    final a = <int>[?e];
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "[?e", 0) + 1, 1)],
        "a promoted read makes the element `?` dead; got: {:?}",
        report.diagnostics
    );
}
