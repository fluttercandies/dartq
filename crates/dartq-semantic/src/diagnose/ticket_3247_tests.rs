//! #3247 pins — the assignability faces carved out of the #3221 recon:
//! the write-target face (`getWriteType` over a bare name) and the
//! type-variable SOURCE arms of the subtype relation.
//!
//! Two upstream faces decide the rows this ticket owns:
//!
//! * The assignment write type is the WRITE ELEMENT's type, never the read's
//!   (`AssignmentExpressionImpl.writeType`, upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart:361
//!   — `_checkForInvalidAssignment(node.writeType!, ...)`, the same call `??=`
//!   shares at :295-296). For a getter/setter pair, an implicit-this member,
//!   or a top-level variable the write face is the `name=` setter's value
//!   formal / the variable's declared type — `b ??= theA` with
//!   `B? get b` + `void set b(B? value)` asks `A <: B?`, not `A <: <the
//!   getter's read row>`. `Engine::assignment_target_ty` re-derives that face
//!   from the name's resolution instead of trusting the push-time snapshot.
//! * A type variable's subtype relation is its BOUND's (subtype.dart's
//!   Right Object `X0` leg, :82-131, and the Left Type Variable Bound arm,
//!   :336-350): `T extends num <: num`, `T extends num` not `<: String`,
//!   and `S extends T <: T`. `Engine::assignable` now runs that arm, so a
//!   bounded parameter source no longer rides the under-report `None` (and
//!   the bound-chained write into a parameter target no longer over-reports
//!   through the interface refusal).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

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

/// —— the write-target face (`??=` / `=`) ————————————————————————————

/// A top-level getter/setter pair: `b ??= theA` checks the setter's value
/// formal (`B?`), so the `A` right-hand side is the oracle's
/// `invalid_assignment` (`A value can't be assigned to B?`) anchored at the
/// operand. Before #3247 the bare read's push-time snapshot left the target
/// unresolved and the row never staged.
#[test]
fn top_level_setter_pair_drives_the_if_null_write_face() {
    let fixture = "\
class A {}
class B extends A {}
A theA = B();
B? get b => null;
void set b(B? value) {}
void main() {
  b ??= theA;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "theA;", 0)],
        "the setter's `B?` write type rejects the `A` operand: {:?}",
        report.diagnostics
    );
}

/// The clean twin: `null` fits `B?` (`Null <: B?` — the Right Nullable
/// arm's `Null` leg), so the same write target reports nothing. This is the
/// false-positive guard for the re-derived write face.
#[test]
fn top_level_setter_pair_accepts_the_null_operand() {
    let fixture = "\
class A {}
class B extends A {}
B? get b => null;
void set b(B? value) {}
void main() {
  b ??= null;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        Vec::<u32>::new(),
        "`null` assigns into `B?`: {:?}",
        report.diagnostics
    );
}

/// A getter-only name is no write target at all: upstream's write element is
/// absent (the `InvalidType` write type accepts every source, and
/// `undefined_setter` owns the report), so `g = "s"` over `int get g` must
/// not emit `invalid_assignment`. Before #3247 the getter's declared `int`
/// was used as the write type, turning the row into a false rejection.
#[test]
fn getter_only_name_has_no_assignability_write_face() {
    let fixture = "\
int get g => 0;
void main() {
  g = \"s\";
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        Vec::<u32>::new(),
        "a getter-only write reports through `undefined_setter`: {:?}",
        report.diagnostics
    );
}

/// The implicit-this member write (`b = A()` inside the class declaring
/// `B? b`) resolves through the field's own declared type — the same
/// `member_write_ty` face `this.b = A()` takes.
#[test]
fn implicit_this_field_write_target_rejects_the_wider_source() {
    let fixture = "\
class A {}
class B extends A {}
class C {
  B? b;
  void test() {
    b = A();
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "A();", 0)],
        "the field's `B?` write type rejects `A`: {:?}",
        report.diagnostics
    );
}

/// —— the type-variable source arms ————————————————————————————————

/// `T extends num` is the Left Type Variable Bound face: `String s = t` asks
/// `num <: String` (false — the report), while `num n = t` asks `num <: num`
/// (true — silent). Before #3247 both stayed the under-report `None`: the
/// parameter head resolved to no container and the walk gave up.
#[test]
fn bounded_parameter_source_answers_through_its_bound() {
    let fixture = "\
void f<T extends num>(T t) {
  String s = t;
  num n = t;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        vec![at(fixture, "= t;", 0) + 2],
        "the bound `num` rejects `String` and accepts `num`: {:?}",
        report.diagnostics
    );
}

/// The bound-chained parameter write (`S extends T`, `T x = s`) is the
/// reflexive arm: `bound(S)` IS the target `T`, so the pair subtypes. The
/// interface refusal that used to catch every distinct parameter source
/// would have reported here.
#[test]
fn bound_chained_parameter_write_is_clean() {
    let fixture = "\
class A {}
void f<T, S extends T>(S s) {
  T x = s;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        Vec::<u32>::new(),
        "`S extends T` writes into `T`: {:?}",
        report.diagnostics
    );
}

/// The residual the arm deliberately leaves open: an UNWRITTEN bound (the
/// implicit `Object?`) keeps the matrix's undecided answer rather than
/// guessing the bound, so `String s = t` over a bare `T` stays silent
/// (the oracle reports `Object? <: String` false). Lifting this needs the
/// implicit bound spelled into the capture, not a guess at the ask site.
#[test]
fn unbounded_parameter_source_stays_undecided() {
    let fixture = "\
void f<T>(T t) {
  String s = t;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "invalid_assignment"),
        Vec::<u32>::new(),
        "an unwritten bound keeps the under-report direction: {:?}",
        report.diagnostics
    );
}
