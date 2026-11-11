//! #3874 lane V pins — the compound-value face, the promoted reads, the
//! numeric ladder's bottom guard, and the tear-off binder:
//!
//! Each pin mirrors a census family of the wave remainder (the
//! `invalid_assignment`/`undefined_operator` recon
//! `census-20261104-092425`), pinned at the smallest fixture the oracle
//! decides. The acceptance twins for three of these live in
//! `ticket_3878_tests.rs` (the routing lane's seam pins, un-ignored as
//! the mechanisms landed); the pins here cover the faces that lane's
//! record routed without a seam pin of their own.
//!
//! Routed record (every mechanism accounted, zero silent drops):
//!
//! * compound tail retype (`AssignmentExpressionImpl.staticType`) —
//!   acceptance pin `compound_result_row_carries_the_operator_result`
//!   (routed file), plus the `??`-composite twin here; both gated on the
//!   `end_variable_initializer` compound window arm (listener lane).
//! * compound promoted receiver read — acceptance pin
//!   `compound_read_uses_the_promoted_receiver` (routed file, pass-2
//!   operator face).
//! * numeric-ladder bottom guard (`!s.isBottom`,
//!   `type_system.dart:2029/:2036`) — acceptance pin
//!   `never_operand_keeps_the_ladder_at_num`, green (the consult
//!   landed).
//! * conditional arm promotion — acceptance pin
//!   `conditional_else_branch_reads_the_promotion` (routed file), plus
//!   the promoted-binary-operand family here (7 census rows); both
//!   gated on the pass-phase latch fix (lane AD, #3897).
//! * tear-off binder display (`T Function<T>(T)`) — the
//!   `functionInstantiation` census family here, green.
//! * the written `A<int, dynamic>` creation row (#3879's routed probe) —
//!   the `downwardsInferenceOnInstanceCreations` family here, green.
//! * the plain index-write's value formal (`x[k] = v` checks `v`
//!   against the resolved `[]=`'s second formal,
//!   `error_detection_helpers.dart:260-289`) — the
//!   `InferredTypeTest/test_mapLiterals` family here, green.

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

/// The compound's value face joins outward through a `??` composite:
/// `x = d += i1 ?? 0` types the whole composite `double` (the operator
/// result is the composite's left face), so the `int` sink refuses.
/// Gated on the same listener half as the plain twin
/// (`compound_result_row_carries_the_operator_result`): the
/// `assignability.rs` tail retype landed, the `end_variable_initializer`
/// compound window arm is the missing half.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn compound_result_joins_through_the_if_null_composite() {
    let report = report_ops(
        "\
void f() {
  double d = 1.0;
  int i1 = 2;
  var x = d += i1 ?? 0;
  int w = x;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the composite reads double; the int sink must refuse it, got {text:?}"
    );
    assert!(
        text[0].contains("'double'"),
        "the composite's value face must be double, got {text:?}"
    );
}

/// The promoted binary operands: `n is int` promotes `n`, so `n + i`
/// resolves and refines on `int` and the `int` sink stays silent (the
/// declared `num` face would answer `num` and report). Gated on the
/// pass-phase fix: the `fold_binary` operand consults landed in
/// `engine_core.rs` but the listener folds see an empty promotion
/// latch (lane AD, #3897).
#[test]
#[ignore = "pass-phase: the listener folds see an empty promotion latch (consult landed, phase fix is lane AD #3897)"]
fn promoted_binary_operands_resolve_on_the_promotion() {
    let report = report_ops(
        "\
void f(num n, int i) {
  if (n is int) {
    int x = (n + i);
  }
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").is_empty(),
        "the promoted int + int is int, got {:?}",
        texts(&report, "invalid_assignment")
    );
}

/// A generic top-level tear-off's static type spells its own binder: the
/// `f` source face reads `T Function<T>(T)`, so the message renders it
/// beside the target's `U Function<U>(U, int)` (the
/// `functionInstantiation_topLevelVariable_genericContext` census row).
#[test]
fn tear_off_display_spells_the_type_parameter_binder() {
    let report = report_ops(
        "\
T f<T>(T a) => a;
U Function<U>(U, int) foo = f;
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the tear-off is a signature mismatch, got {text:?}"
    );
    assert!(
        text[0].contains("'T Function<T>(T)'"),
        "the source face must spell its binder, got {text:?}"
    );
    assert!(
        text[0].contains("'U Function<U>(U, int)'"),
        "the target face must spell its binder, got {text:?}"
    );
}

/// The written `A<int, dynamic>` creation against an `A<int, String>`
/// target reports the dynamic slot (the covariance leg answers
/// `dynamic <: String` false exactly as the oracle's `isSubtypeOf` — the
/// #3861 dynamic-face fold, re-checked here through R's #3879 probe
/// shape).
#[test]
fn written_dynamic_slot_reports_the_creation_row() {
    let report = report_ops(
        "\
class A<S, T> {
  S x;
  T y;
  A(this.x, this.y);
}
void m() {
  A<int, String> a4 = new A<int, dynamic>(3, 'hello');
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the dynamic slot refuses the String target, got {text:?}"
    );
    assert!(
        text[0].contains("'A<int, dynamic>'") && text[0].contains("'A<int, String>'"),
        "the message spells both instantiations, got {text:?}"
    );
}

/// The index-write's destination is the resolved `[]=`'s VALUE formal,
/// substituted by the receiver's written instantiation
/// (`Map<int, String>` writes against `String`): an `int` value reports
/// at the value span, the matching value stays silent, and the
/// `List<double>` face keeps the int-to-double literal context silent.
#[test]
fn index_write_checks_the_value_against_the_element_formal() {
    let report = report_ops(
        "\
void f() {
  var x = <int, String>{1: 'a'};
  x[3] = 42;
  x[3] = 'b';
  var y = <double>[];
  y[0] = 1;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "exactly the int-into-String value mismatch, got {text:?}"
    );
    assert!(
        text[0].contains("'int'") && text[0].contains("'String'"),
        "the message spells the value and the element formal, got {text:?}"
    );
}

/// The tear-off binder's bound leg (`relateTypeParameters`,
/// subtype.dart:338-345): a generic tear-off relates to a generic target
/// only when the bounds relate MUTUALLY — `T extends int` into
/// `U extends num` refuses (the reverse bound direction fails) exactly
/// as the unbounded-into-bounded face does. The #3896 silent-accept
/// family's variable-assignment half; the call-site half is the
/// invocation lane's (#3896/B4).
#[test]
fn tear_off_bounds_relate_mutually() {
    let report = report_ops(
        "\
void f<T extends int>(T a) {}
void m() {
  void Function<U extends num>(U) g = f;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the reverse bound direction refuses the pair, got {text:?}"
    );
    assert!(
        text[0].contains("'void Function<T extends int>(T)'"),
        "the message spells the source binder, got {text:?}"
    );
}
