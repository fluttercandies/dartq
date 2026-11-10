//! #3310 pins — `invalid_null_aware_operator` receiver-nullability faces.
//!
//! The frozen verdict is upstream `_checkForUnnecessaryNullAware`'s
//! `isStrictlyNonNullable` gate (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/error_verifier.dart:7668-7718): the operator
//! is dead when the receiver's static type is strictly non-nullable. The
//! operator spelling picks the message and the replacement
//! (`?.`/`?..`/`?[`/`?...`), and the anchor is always the operator token
//! alone — never the receiver.
//!
//! These pins cover the shapes the current verdict already answers
//! correctly, so the dominant arms cannot regress while the known
//! divergences are fixed in their owning (live-lane) files:
//! - the static-qualifier face (`C?.x`, `C?.x = 0`, an alias head) reports
//!   with no type question at all;
//! - declared non-nullable receivers report in every operator form;
//! - declared nullable receivers stay silent in every operator form;
//! - an instance getter on a non-nullable class receiver reports, and an
//!   extension member on a non-nullable receiver reports;
//! - a bounded type-parameter receiver decides through its written bound
//!   (`T extends Object` reports, `T`, `T extends Object?` stay silent);
//! - a cascade reports only its first null-aware section's operator;
//! - a `?` collection element decides by the element's own type.
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
    let hit = from
        + fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture");
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

/// The `?.`-family message (the `{0}` operator and `{1}` replacement slots).
fn qdot_message(op: &str) -> String {
    format!("The receiver can't be null, so the null-aware operator '{op}' is unnecessary.")
}

// —— static-qualifier face ————————————————————————————————————————————————

/// `C?.x` on a class name: the receiver IS the type, so the operator can
/// never be meaningful — reported unconditionally at the `?.` token.
#[test]
fn static_qualifier_getter_reports_the_unnecessary_qdot() {
    let fixture = "\
class C {
  static int x = 0;
}

void f() {
  C?.x;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![qdot_message("?.")],
        "`C?.x` reports once; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "C?.x", 0) + 1, 2)],
        "the anchor is the `?.` token"
    );
}

/// `C?.x = 0` on a class name: the assignment target's static qualifier is
/// the same unconditional face.
#[test]
fn static_qualifier_setter_reports_the_unnecessary_qdot() {
    let fixture = "\
class C {
  static int x = 0;
}

void f() {
  C?.x = 0;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "C?.x", 0) + 1, 2)],
        "`C?.x = 0` reports at the `?.` token; got: {:?}",
        report.diagnostics
    );
}

/// A type alias keeps the static qualifier: `B?.foo()` where `B = A` is the
/// same type-name receiver face.
#[test]
fn aliased_static_qualifier_reports_the_unnecessary_qdot() {
    let fixture = "\
class A {
  static void foo() {}
}

typedef B = A;

void f() {
  B?.foo();
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "B?.foo", 0) + 1, 2)],
        "`B?.foo()` reports at the `?.` token; got: {:?}",
        report.diagnostics
    );
}

// —— declared non-nullable / nullable receivers ————————————————————————————

/// Every operator form over a declared non-nullable receiver reports, each
/// with its own spelling-driven message and its own token anchor.
#[test]
fn non_nullable_receivers_report_every_operator_form() {
    let fixture = "\
void f(int x, List<int> l) {
  x?.isEven;
  x?..isEven;
  l?[0];
  [...?l];
}
";
    let report = report_ops(fixture);
    let mut expected = vec![
        qdot_message("?."),
        qdot_message("?.."),
        qdot_message("?["),
        qdot_message("?..."),
    ];
    expected.sort();
    let mut got = messages(&report);
    got.sort();
    assert_eq!(
        got, expected,
        "every form reports; diagnostics: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![
            (at(fixture, "?.", 0), 2),
            (at(fixture, "?..", 0), 3),
            (at(fixture, "?[", 0), 2),
            (at(fixture, "...?", 0), 4),
        ],
        "each anchor is its own operator token"
    );
}

/// The silent-negative twin: a declared nullable receiver keeps every
/// operator form meaningful.
#[test]
fn nullable_receivers_keep_every_operator_form_silent() {
    let fixture = "\
void f(int? x, List<int>? l) {
  x?.isEven;
  x?..isEven;
  l?[0];
  [...?l];
}
";
    let report = report_ops(fixture);
    assert!(
        messages(&report).is_empty(),
        "a nullable receiver keeps the `?` meaningful; got: {:?}",
        report.diagnostics
    );
}

/// An instance getter on a non-nullable class receiver reports — the
/// selector's declared non-null receiver type is the verdict's input.
#[test]
fn instance_getter_on_non_nullable_receiver_reports() {
    let fixture = "\
class C {
  int get v => 0;
}

void f(C c) {
  c?.v;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "c?.v", 0) + 1, 2)],
        "`c?.v` on a non-nullable `c` reports; got: {:?}",
        report.diagnostics
    );
}

/// An extension member resolved against a non-nullable receiver reports the
/// same way as an interface member.
#[test]
fn extension_member_on_non_nullable_receiver_reports() {
    let fixture = "\
extension E on int {
  int get v => 0;
}

void f(int x) {
  x?.v;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "x?.v", 0) + 1, 2)],
        "`x?.v` through an extension reports; got: {:?}",
        report.diagnostics
    );
}

// —— bound-aware type-parameter face ————————————————————————————————————————

/// A type-parameter receiver decides through its written bound: a
/// non-nullable bound reports, the implicit `Object?` stays silent.
#[test]
fn type_parameter_receiver_decides_through_its_bound() {
    let fixture = "\
void nonNullBound<T extends Object>(T x) {
  x?.toString();
}

void implicitBound<T>(T y) {
  y?.toString();
}

void nullableBound<T extends Object?>(T z) {
  z?.toString();
}
";
    let report = report_ops(fixture);
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "x?.toString", 0) + 1, 2)],
        "only the non-nullable bound reports; got: {:?}",
        report.diagnostics
    );
}

// —— cascade face ———————————————————————————————————————————————————————————

/// A null-aware cascade reports only its first section's operator: the
/// following `..p` sections are never null-aware.
#[test]
fn cascade_reports_only_its_first_section() {
    let fixture = "\
class C {
  int get p => 0;
}

void f(C c) {
  c?..p..p;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![qdot_message("?..")],
        "one row, the `?..` section; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "c?..p", 0) + 1, 3)],
        "the anchor is the `?..` token"
    );
}

// —— collection-element face ————————————————————————————————————————————————

/// A `?` collection element over a non-nullable local reports at the bare
/// `?` token; the nullable twin stays silent.
#[test]
fn null_aware_element_decides_by_the_element_type() {
    let fixture = "\
void f() {
  int e1 = 42;
  int? e2 = 1 > 2 ? 2 : null;
  var l = [
    ?e1,
    ?e2,
  ];
}
";
    let report = report_ops(fixture);
    assert_eq!(
        messages(&report),
        vec![
            "The element can't be null, so the null-aware operator '?' is unnecessary.".to_string(),
        ],
        "only the non-nullable element reports; got: {:?}",
        report.diagnostics
    );
    assert_eq!(
        anchors(&report),
        vec![(at(fixture, "?e1", 0), 1)],
        "the element face anchors the `?` token alone"
    );
}
