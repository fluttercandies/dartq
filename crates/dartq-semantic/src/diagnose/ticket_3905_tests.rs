//! #3905 lane V pins — the class-wave remainder faces this lane owns:
//!
//! Each pin mirrors a census family of the class wave (the
//! `invalid_assignment` recon `census-20261104-092425`, the H5/J3/G1/R2/
//! J1/H4/C2/A2/E1 routing in the wave's dispatch record), pinned at the
//! smallest fixture the oracle decides. The families whose root faces
//! live outside this file are recorded in the lane's report with their
//! owning layer; the pins here hold the faces `assignability.rs`
//! decides.
//!
//! * the compound index-write face (`c[k] op= e`): the augmented
//!   operator resolves on the READ element and the write-back verdict
//!   asks the `[]=` value formal — the two-row shape (the operator's
//!   argument refusal beside the write-back refusal, both anchored at
//!   the right operand). Upstream `assignment_expression_resolver.dart`
//!   :368-372 + `_checkForInvalidAssignment(writeAcceptedType, ...)`;
//!   the value formal is `error_detection_helpers.dart:260-289`'s
//!   `writeType`.
//! * the #3896 semantic-side binder face: an unbounded generic tear-off
//!   into a bounded generic target reports (the `relateTypeParameters`
//!   bound gate, upstream `subtype.dart:338-345` — a missing bound
//!   meets a written bound only as the implicit `dynamic`, and the
//!   mutual bound relation refuses).
//! * the field-initializer Null face (`: this.x = null` over a
//!   non-nullable field): the initializer-list write verdict — the
//!   oracle's `field_initializer_not_assignable`.
//! * the instantiated-supertype-context creation
//!   (`D<Object> f = new G()` where `G implements D<A>`): the
//!   mixin-application's interface edge accepts through the declared
//!   walk — the oracle stays silent.

use super::test_rig::report_ops;
use crate::FileReport;

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

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// The compound index-write resolves the augmented operator on the read
/// element and checks the write-back against the `[]=` value formal:
/// `x[0] += 1.5` over a `List<int>` reports exactly `double -> int` at
/// the `1.5` span, and a user class's `b[0] += "s"` reports the
/// operator's argument refusal (`String -> num`) BESIDE the write-back
/// refusal (`num -> int`) — both at the right operand.
#[test]
fn compound_index_write_checks_the_element_faces() {
    let report = report_ops(
        "\
void m() {
  var x = <int>[];
  x[0] += 1.5;
}
class Box {
  int operator [](int i) => 0;
  void operator []=(int i, int v) {}
}
void n(Box b) {
  b[0] += 's';
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        2,
        "the SDK and the user-class write-back refusals, got {text:?}"
    );
    assert!(
        text[0].contains("'double'") && text[0].contains("'int'"),
        "the SDK face refuses double into the int element, got {text:?}"
    );
    assert!(
        text[1].contains("'num'") && text[1].contains("'int'"),
        "the user face refuses the operator result into the value formal, got {text:?}"
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert!(
        atna.iter().any(|t| t.contains("'String'")),
        "the user operator's argument refusal rides beside the write-back, got {atna:?}"
    );
    // Routed residual (recorded, not asserted): the numeric argument
    // face's PARAMETER spelling answers `int` where the oracle spells
    // `num` — the core facade's `int operator +` formal capture — and
    // the `double`-fitting-`num` silence follows from the same face.
    // The plain compound path carries the identical divergence today
    // (`i += 2.0`), so the index face inherits it uniformly; the
    // operator-signature capture owns the fix, not this file.
}

/// A matching compound index-write stays silent: `y[1] += 1` over a
/// `List<double>` reads `int + double -> double` and writes back into
/// the `double` element clean (the int literal's downward context), and
/// the user class's element faces accept their own result.
#[test]
fn compound_index_write_matching_faces_stay_silent() {
    let report = report_ops(
        "\
void m() {
  var y = <double>[];
  y[1] += 1;
}
class Box {
  int operator [](int i) => 0;
  void operator []=(int i, int v) {}
}
void n(Box b) {
  b[2] += 3;
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").is_empty(),
        "the matching faces stay silent, got {:?}",
        texts(&report, "invalid_assignment")
    );
    assert!(
        offsets(&report, "argument_type_not_assignable").is_empty(),
        "the operator's argument accepts, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

/// The #3896 semantic-side binder face: an UNBOUNDED generic tear-off
/// into a bounded generic target reports (`T Function<T>(T)` against
/// `void Function<U extends int>(U)` — the bound gate fires before the
/// bodies, the oracle's verdict).
#[test]
fn unbounded_tear_off_refuses_the_bounded_target() {
    let report = report_ops(
        "\
void f<T>(T a) {}
void m() {
  void Function<U extends int>(U) g = f;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the unbounded binder misses the bounded target, got {text:?}"
    );
    assert!(
        text[0].contains("'void Function<T>(T)'"),
        "the source face spells its unbounded binder, got {text:?}"
    );
}

/// The field-initializer Null face: `: this.x = null` over a
/// non-nullable `String` field reports the initializer-list write
/// verdict (the oracle's `field_initializer_not_assignable`, anchored
/// at the initializer expression).
#[test]
fn this_initializer_null_refuses_the_field_face() {
    let report = report_ops(
        "\
class A {
  String x = 's';
  A(String? s) : this.x = null;
}
",
    );
    let text = texts(&report, "field_initializer_not_assignable");
    assert_eq!(
        offsets(&report, "field_initializer_not_assignable").len(),
        1,
        "Null never fits the String field, got {text:?}"
    );
    assert!(
        text[0].contains("'Null'") && text[0].contains("'String'"),
        "the message spells the initializer and the field face, got {text:?}"
    );
}

/// A mixin application's interface edge accepts its instantiated
/// supertype context: `D<Object> f = new G()` where `G implements
/// D<A>` — `A <: Object` through the covariant walk, the oracle stays
/// silent.
#[test]
fn mixin_application_interface_edge_accepts() {
    let report = report_ops(
        "\
class C {}
mixin E {}
abstract class D<T> {}
class G = C with E implements D<A> {}
class A {}
void m() {
  D<Object> f = new G();
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").is_empty(),
        "the implements edge carries G into D<Object>, got {:?}",
        texts(&report, "invalid_assignment")
    );
}

/// The M4 last-mile face (the i2b fallback's argument verdict): the
/// failed inference fills the formal's parameter with its i2b default
/// (`U := dynamic`), and the argument ask `A<int>` against
/// `B<dynamic>` must REPORT — `A` is `B`'s SUPERtype (`B<S> extends
/// A<S>`), and the verdict never unfolds the TARGET's supertype to
/// accept the source through it (upstream `subtype.dart`'s interface
/// case walks the SOURCE's supertype instantiations only). The
/// oracle's `argument_type_not_assignable` at the argument span. The
/// `ticket_3880_tests.rs` evidence pin carries the same fixture
/// `#[ignore]`d for the owning lane to un-gate.
///
/// Routed (#3917 discriminator, an env-gated ask probe at the
/// `Engine::assignable` entry, since stripped): the only A/B-headed ask
/// for this fixture is
/// `B<dynamic> -> B<dynamic>` — the ARGUMENT side is captured as the
/// formal's own i2b-substituted face, so the verdict sees identical faces
/// and accepts. `assignable` is exonerated (identical faces accept
/// correctly). Owning lane: invocation — the creation-argument capture
/// must read the argument's own static type the way the sibling walks do
/// (`check_named_creation_arguments` feeds the raw parse-time face; the
/// super walk's `rebound_read_face`/`read_type_at` outranking and the
/// enum walk's are the pattern — the const local's declared `A<int>` read
/// face makes the ask `A<int> -> B<dynamic>`, which refuses). This pin is
/// the fail-before evidence; it flips green when the capture lands.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn creation_i2b_fallback_argument_reports_through_the_formal() {
    let report = report_ops(
        "\
class A<T> {
  const A();
}
class B<S> extends A<S> {
  const B();
}
class C<U> {
  final A<U> a;
  const C.subtyped(B<U> this.a);
}
void main() {
  const A<int> a = const B<int>();
  const C.subtyped(a);
}
",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert!(
        atna.iter()
            .any(|t| t.contains("'A<int>'") && t.contains("'B<dynamic>'")),
        "the i2b fallback's argument ask must report, got {atna:?}"
    );
}
