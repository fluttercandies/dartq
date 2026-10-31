//! #3271 pins — the SETTER-ONLY implicit-`this` write face.
//!
//! `class C { set only(T v) {} }` + `only = e` inside the body is a write
//! whose write element is the `name=` setter: upstream resolves the
//! unqualified assignment's left side through the write namespace
//! (`preferSetter`) and checks the right operand against that element's
//! type — `AssignmentExpressionImpl.writeType`
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! assignment_expression_resolver.dart:54/:83, `_checkForInvalidAssignment`
//! at :361). The setter's type IS its single value formal, so a source that
//! does not subtype it reports `invalid_assignment`.
//!
//! dartq's row model loses that face: the tentative read at push time
//! consults the GETTER namespace, and a setter-only name has no entry there,
//! so the row carries `BareInfo::Missing` and no declared type. The
//! getter/setter PAIR shape keeps a `Plain` face (the getter's) and re-derives
//! the write face from the member walk, which is why only the setter-only
//! spelling under-reported. [`Engine::bare_write_formal_ty`]
//! (`row_resolution.rs`) answers the missing face — the same own-container
//! `name=` hit under the same accessibility gate as the
//! [`Engine::bare_write_target_exists`] rescue — and
//! [`Engine::assignment_target_ty`] (`assignability.rs`) reads it for a
//! plain `=` write.
//!
//! The pins below lock the invariant trio:
//! **setter-only rejects a mismatched source, pair and `this.` spellings
//! answer the same face, and a getter-only name asks no assignability
//! question at all** (its write element does not exist — the erroneous-write
//! family owns that report, not `invalid_assignment`).
//!
//! Out of scope, recorded: the TOP-LEVEL setter-only twin (`set only(T v)`
//! outside any container, `only = e` in a top-level function) resolves the
//! same way upstream but still answers no write face here — a separate
//! residual, not pinned to silence.
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

/// The gap this ticket closes: a setter-only implicit-`this` write checks
/// its right side against the setter's value formal, so an unrelated source
/// reports at the right operand.
#[test]
fn setter_only_implicit_this_write_rejects_a_mismatched_source() {
    let fixture = "\
class A {}
class SetterOnly {
  set only(A v) {}
  void put(String s) {
    only = s;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= s;", 0) + 2],
        "the `name=` setter's `A` formal is the write type: {:?}",
        report.diagnostics
    );
}

/// The accept arm: a source that subtypes the setter's formal stays clean.
/// This is the direction a naive "report every setter-only write" fix gets
/// wrong, so the pin guards against a false positive.
#[test]
fn setter_only_implicit_this_write_accepts_the_formal() {
    let fixture = "\
class A {}
class SetterOnly {
  set only(A v) {}
  void put(A a) {
    only = a;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        Vec::<u32>::new(),
        "`A` into the setter's `A` formal is clean: {:?}",
        report.diagnostics
    );
}

/// The explicit-`this` spelling answers the same face through the member
/// arm; the bare spelling must not diverge from it.
#[test]
fn setter_only_explicit_this_write_reports_the_same_face() {
    let fixture = "\
class A {}
class SetterOnly {
  set only(A v) {}
  void put(String s) {
    this.only = s;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= s;", 0) + 2],
        "`this.only = s` asks the same setter formal: {:?}",
        report.diagnostics
    );
}

/// The regression arm the ticket names: the getter/setter PAIR keeps
/// reporting — the new setter-only arm must not disturb the `Plain`
/// (getter-faced) path that already re-derives the write face.
#[test]
fn pair_shape_write_still_reports() {
    let fixture = "\
class A {}
class Pair {
  late A stored;
  A get only => stored;
  set only(A value) {
    stored = value;
  }
  void put(String s) {
    only = s;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= s;", 0) + 2],
        "the pair's setter formal is the write type: {:?}",
        report.diagnostics
    );
}

/// A getter-only name declares no setter element, so the write asks no
/// assignability question: upstream's write element is `InvalidType`
/// (which accepts every source) and the erroneous-write family owns the
/// report. The check must stay silent rather than resolving to the GETTER's
/// face.
#[test]
fn getter_only_implicit_this_write_stays_off_the_assign_check() {
    let fixture = "\
class A {}
class GetterOnly {
  A get only => throw 0;
  void put(String s) {
    only = s;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        Vec::<u32>::new(),
        "a getter-only write resolves no setter element: {:?}",
        report.diagnostics
    );
}
