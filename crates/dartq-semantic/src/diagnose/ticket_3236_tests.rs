//! #3236 pins — the name-level can-be relation's four model gaps.
//!
//! `NameWorld::can_be_subtype_of` ports `TypeSystemImpl.canBeSubtypeOf`
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:138-323). Three of
//! its arms were missing at name level, and the exhaustiveness row model
//! manufactured an unknown space for every object pattern carrying a
//! constant field:
//!
//! 1. the same-declaration pair compares its type argument lists pairwise
//!    (:201-219 `canBeSubtypeOfInterfaces`), so `Box<int>` and `Box<S>` do
//!    not collapse into one shape;
//! 2. `FutureOr<T>` splits into `T` / `Future<T>` on either side
//!    (:173-189), so a disjoint required/matched type still decides;
//! 3. the closed-set scans read `allSubtypes != null`
//!    (pkg/analyzer/lib/src/dart/element/element.dart:159-217), which a
//!    `final` class and every dart:core primitive answer — the dominant
//!    firing arm of `pattern_never_matches_value_type`;
//! 4. an object pattern whose field sub-pattern is a `true`/`false`/enum
//!    constant keeps its property column (`convertObjectPattern`'s field
//!    spaces) instead of degrading to the unknown space.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

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

/// The never-match family's rows, as `(offset, code)` pairs — the whole
/// family, so a verdict that lands on the wrong kind still shows.
fn never_matches(report: &FileReport) -> Vec<u32> {
    offsets(report, "pattern_never_matches_value_type")
}

/// Same-declaration, disagreeing type arguments: `Box<S>` can hold no
/// `Box<int>` (value_system.dart's pairwise argument ask,
/// type_system.dart:201-219 — `canBeSubtypeOf(S, int)` is false, `S` being
/// an open class and `int` a closed one). Before #3236 the same-head branch
/// answered `true` without consulting the arguments, so the pattern stayed
/// silent.
#[test]
fn same_head_argument_mismatch_reports() {
    let fixture = "\
class Box<T> {}
class S {}
void f(Box<S> v) {
  switch (v) {
    case Box<int> x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        vec![at(fixture, "Box<int>", 0)],
        "`Box<S>` can never match the required `Box<int>`: {:?}",
        report.diagnostics
    );
}

/// Same head, same arguments: the pairwise ask is the identity, so the
/// pattern keeps its pre-#3236 silence (the argument walk must not turn
/// every instantiated pattern into a never-match).
#[test]
fn same_head_same_arguments_stay_live() {
    let fixture = "\
class Box<T> {}
void f(Box<int> v) {
  switch (v) {
    case Box<int> x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        Vec::<u32>::new(),
        "`Box<int>` matches `Box<int>`: {:?}",
        report.diagnostics
    );
}

/// A bare head stays the wildcard it always was: the instantiate-to-bound
/// spelling fits every instantiation, so the argument walk declines the
/// pair instead of demanding an argument list on both sides.
#[test]
fn bare_head_stays_the_wildcard() {
    let fixture = "\
class Box<T> {}
void f(Box<int> v) {
  switch (v) {
    case Box x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        Vec::<u32>::new(),
        "the bare `Box` head is the wildcard side: {:?}",
        report.diagnostics
    );
}

/// The dominant `allSubtypes != null` arm: `int` is a `final` class in
/// dart:core, so its instance set is complete and a `String` requirement is
/// decisively disjoint (element.dart:159-217; type_system.dart:236-250
/// answers `false` outright for a closed left). Before #3236 neither side
/// is sealed, so the pair fell to the conservative `true`.
#[test]
fn core_primitive_scrutinee_decides_against_a_class_case() {
    let fixture = "\
void f(int v) {
  switch (v) {
    case String x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        vec![at(fixture, "String x", 0)],
        "an `int` value can never be a `String`: {:?}",
        report.diagnostics
    );
}

/// The mirror direction of the same arm, and the core chain's candidate
/// seed: `num` is closed too, and its candidate set IS `int`/`double`, so
/// a `case num n:` over an `int` scrutinee must stay live (the oracle's
/// `allSubtypes` = `[int, double]` scan finds `int` immediately).
#[test]
fn num_requirement_over_an_int_scrutinee_stays_live() {
    let fixture = "\
void f(int v) {
  switch (v) {
    case num x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        Vec::<u32>::new(),
        "an `int` value is a `num`: {:?}",
        report.diagnostics
    );
}

/// `FutureOr<T>` on the LEFT of the ask (`matchedValueType`, the official
/// call's first argument — resolver.dart's
/// `canBeSubtypeOf(matchedValueType, requiredType)`): the union splits into
/// `T | Future<T>` and neither half can be a `String`
/// (type_system.dart:173-183 plus the closed-`String` `false` of the
/// future half).
#[test]
fn future_or_matched_type_splits_against_a_closed_requirement() {
    let fixture = "\
void f(FutureOr<int> v) {
  switch (v) {
    case String x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        vec![at(fixture, "String x", 0)],
        "`FutureOr<int>` is `int | Future<int>`, never a `String`: {:?}",
        report.diagnostics
    );
}

/// `FutureOr<T>` on the RIGHT of the ask (the required type): a `String`
/// value is neither a `T` nor a `Future<T>`, and the required side's own
/// closedness never rescues it — `String`'s instance set is complete
/// (type_system.dart:184-189).
#[test]
fn future_or_required_type_splits_against_a_closed_matched_type() {
    let fixture = "\
void f(String v) {
  switch (v) {
    case FutureOr<int> x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        vec![at(fixture, "FutureOr<int>", 0)],
        "a `String` is neither `int` nor `Future<int>`: {:?}",
        report.diagnostics
    );
}

/// Both halves of the union keep their own verdict: the base (`int`) and
/// the future (`Future<int>`) each match the `FutureOr<int>` scrutinee, so
/// the split must not turn either into a never-match.
#[test]
fn future_or_halves_stay_live() {
    let fixture = "\
void f(FutureOr<int> v) {
  switch (v) {
    case int x: x; break;
    case Future<int> y: y; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        Vec::<u32>::new(),
        "both `FutureOr<int>` halves match the scrutinee: {:?}",
        report.diagnostics
    );
}

/// A `final` class's instance set is confined to its library, so
/// `allSubtypes` is non-null for it too (element.dart:159-181) and a case
/// requiring an unrelated open interface is decisively dead. A `sealed`
/// class would answer the same arm; the `final` one is the gap #3243
/// folded in.
#[test]
fn final_class_scrutinee_decides_against_an_interface_case() {
    let fixture = "\
final class F {}
class I {}
void f(F v) {
  switch (v) {
    case I x: x; break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        never_matches(&report),
        vec![at(fixture, "I x", 0)],
        "a `final` class closes its subtype set: {:?}",
        report.diagnostics
    );
}

/// An object pattern's constant field keeps its property column
/// (`convertObjectPattern`'s per-field spaces): two `C(b: true)` cases
/// cover the same values, so the second is unreachable. Before #3236 a
/// `true`/`false`/enum leaf degraded the whole pattern to the unknown
/// space, and two identical cases never covered one another.
#[test]
fn constant_field_object_cases_cover_one_another() {
    let fixture = "\
class C {
  final bool b;
  const C(this.b);
}
void f(C c) {
  switch (c) {
    case C(b: true): break;
    case C(b: true): break;
    default: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "unreachable_switch_case"),
        vec![at(fixture, "case C(b: true)", 1)],
        "the duplicate `C(b: true)` case is unreachable: {:?}",
        report.diagnostics
    );
}
