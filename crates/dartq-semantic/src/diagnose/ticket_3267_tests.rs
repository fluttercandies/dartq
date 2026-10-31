//! #3267 pins — the type-parameter member-write SLOT face of
//! [`Engine::member_write_ty`] (`resolution.rs`).
//!
//! Upstream decides an assignment against the write element's own type
//! (`AssignmentExpressionImpl.writeType`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart:361)
//! with `isSubtypeOf(rightType, writeType)`. For a member whose declared
//! type is a bounded type parameter (`class Holder<T extends A> { T slot; }`)
//! that write type is the PARAMETER `T`, never its bound `A`: a plain type
//! variable on the right-hand side of the subtype relation falls through
//! every interface arm and reaches `return false`
//! (pkg/analyzer/lib/src/dart/element/subtype.dart:175-249 — "Right
//! Promoted Variable" only fires for a promoted variable, and the "Left
//! Type Variable Bound" arm at :259-274 needs the SOURCE to be a type
//! variable). The only true outcomes are the reflexivity arm (`T <: T`),
//! `Null <: T?`, and the Left Top leg — so `B <: T` reports even though
//! `B <: A`, and `int <: T` reports for `T extends num` ("the bound rescues
//! nothing", probed 3.13.1 in #3247).
//!
//! The pins below therefore lock the invariant `member_write_ty` must keep:
//! **the member slot answers the declared type-parameter face, never a
//! bound-collapsed one.** Collapsing the slot to `A` would silence the first
//! pin (a bound-subtype source would then subtype the slot) and `int` would
//! satisfy `T extends num` — both divergences from the oracle. Bound
//! instantiation belongs to the RECEIVER face instead (`C<B>().slot = e`
//! substitutes the written argument; a raw `C` is `C<bounds>`), which the
//! assignability pass owns.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (the module
//! lives outside this ticket's write scope).

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

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

/// A member write whose declared type is a bounded parameter reports the
/// BOUND'S OWN SUBTYPE — the slot is the parameter, so `B <: T` is false
/// (subtype.dart's fall-through) even though `B <: A`. A bound-collapsed
/// slot would answer `B <: A` and stay silent, which is the divergence this
/// pin guards. Both the implicit-this and the `this.` spelling take the same
/// face.
#[test]
fn bounded_param_member_write_rejects_a_bound_subtype_source() {
    let fixture = "\
class A {}
class B extends A {}
class Holder<T extends A> {
  late T slot;
  void put(B b) {
    slot = b;
    this.slot = b;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= b;", 0) + 2, at(fixture, "= b;", 1) + 2],
        "the slot is `T`, so a bound-subtype source reports: {:?}",
        report.diagnostics
    );
}

/// The primitive spelling of the same face: `T extends num` written with an
/// `int` literal reports (`int <: T` is false — `T` could be `double`).
/// This is the pin a bound-collapsed slot fails first: `int <: num` holds.
#[test]
fn bounded_param_member_write_does_not_ride_the_numeric_bound() {
    let fixture = "\
class NumHolder<T extends num> {
  late T slot;
  void put() {
    slot = 3;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= 3;", 0) + 2],
        "`int` does not subtype a bare `T extends num`: {:?}",
        report.diagnostics
    );
}

/// A setter's value formal is the second arm of `member_write_ty`
/// (`name=`'s single positional formal — `getWriteType` never answers the
/// getter's return). The same parameter face reaches the pair there, so the
/// bound-subtype source still reports.
///
/// The fixture spells a getter/setter PAIR: a setter-only name (`set only(T
/// v)` with no getter) never reaches this face through a bare implicit-this
/// write — the push-time read misses it and the row is rescued by
/// `bare_write_target_exists`, which is the row-resolution face's business,
/// not this slot's.
#[test]
fn bounded_param_setter_write_rejects_a_bound_subtype_source() {
    let fixture = "\
class A {}
class B extends A {}
class PairHolder<T extends A> {
  late T stored;
  T get only => stored;
  set only(T value) {
    stored = value;
  }
  void put(B b) {
    only = b;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= b;", 0) + 2],
        "the setter's `T` formal is the write type: {:?}",
        report.diagnostics
    );
}

/// The non-parameter regression: a plain declared member type keeps the
/// ordinary subtype matrix — a bound-ish subtype is clean and a disjoint
/// source reports. The parameter face must not leak into non-param members.
#[test]
fn non_param_member_write_keeps_the_ordinary_matrix() {
    let fixture = "\
class A {}
class B extends A {}
class PlainHolder {
  late A slot;
  void accepts(B b) {
    slot = b;
  }
  void rejects(String s) {
    slot = s;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= s;", 0) + 2],
        "`B <: A` is clean, `String <: A` reports: {:?}",
        report.diagnostics
    );
}

/// The reflexive and nullability arms of a parameter slot stay clean: the
/// same parameter written into itself (`T <: T`, subtype.dart's Type
/// Variable Reflexivity 1) and `null` into `T?` (the Left Null leg's
/// nullable conjunct).
#[test]
fn same_parameter_and_nullable_slot_writes_stay_clean() {
    let fixture = "\
class A {}
class Holder<T extends A> {
  late T slot;
  T? maybe;
  void put(T t) {
    slot = t;
    maybe = null;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        Vec::<u32>::new(),
        "`T` into `T` and `null` into `T?` are both clean: {:?}",
        report.diagnostics
    );
}
