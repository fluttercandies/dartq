//! #3917 lane V pins — packet 3: the mid-flight routings from the four
//! live lanes plus this lane's own M4 residual:
//!
//! * the call-result index-write receiver (`f()[0] = v`): the folded
//!   send's Value row is the walk root — the value formal face runs the
//!   same as every other index write (`error_detection_helpers.dart`
//!   :260-289).
//! * the `??=` index write: the plain assignment face's operator consult
//!   already routes `??=` through the same value-formal ask.
//! * the call bounds face (routed): upstream's regular-bounded check is
//!   the strict `isSubtypeOf` (upstream Dart SDK source,
//!   `pkg/analyzer/lib/src/error/type_arguments_verifier.dart:373`),
//!   never the assignability relaxation — `void` against `num`/`Object`
//!   /a function-typed bound refuses (`subtype.dart`'s Right Object arm
//!   answers false for a `VoidTypeImpl` source). Our verdict site asks
//!   [`Engine::assignable`]; the strict twin is [`Engine::slot_subtype`]
//!   — the rewire is the invocation lane's one-token change.
//! * the `??` composite as a send argument (routed): the argument face
//!   must ask the join-aware answer (`f(a ?? 3)` reads `int`, never
//!   `int?`) — [`Engine::rhs_row_ty`] is the join and is now
//!   `pub(crate)` for the invocation lane's consult.
//! * the S1c promoted-bound await face (routed): `x is X1?` under
//!   `X1 extends Future<Never>` must flatten `await x` to the bare
//!   parameter `X` (upstream `_futureTypeOfBounded(X1?)` consults the
//!   ELEMENT bound, whose nullable gate skips `asInstanceOf`,
//!   `type_system.dart:406-422 + 464-478 + 1845-1854`) — the Ty model
//!   carries no declared-vs-promoted distinction at the read site, so
//!   the fix needs the intersection face in the flow fold or both faces
//!   at the read site.
//! * the `FutureOr<void>` source face: the Left FutureOr conjunction
//!   asks its legs as pure subtypes (upstream `subtype.dart:371-385` —
//!   `FutureOr<S0> <: T1` iff `S0 <: T1` and `Future<S0> <: T1`), and
//!   `void <: Object` is FALSE (Left Void answers the top faces only),
//!   so `Object o = h()` over `FutureOr<void> h()` reports. The legs
//!   consult the strict twin, never the assignment posture whose
//!   `void`-source suppression (`use_of_void_result`'s business) left
//!   the conjunction undecided and the row unreported.

use super::test_rig::report_ops;
use crate::FileReport;

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

/// The call-result receiver: `f()[0] = 2.5` over a `List<int>` return
/// asks the `[]=` value formal through the folded send's own face — the
/// oracle reports `double -> int` at the `2.5` span.
#[test]
fn call_result_index_write_checks_the_value_formal() {
    let report = report_ops(
        "\
List<int> f() => [];
void m() {
  f()[0] = 2.5;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the send's element formal refuses double, got {text:?}"
    );
    assert!(
        text[0].contains("'double'") && text[0].contains("'int'"),
        "the message spells the value and the element formal, got {text:?}"
    );
}

/// The `??=` index write runs the same value-formal ask as the plain
/// and compound forms: `l[0] ??= 2.5` over a `List<int>` reports
/// `double -> int` (the dead-code faces beside it are their own lanes').
#[test]
fn if_null_index_write_checks_the_value_formal() {
    let report = report_ops(
        "\
void g() {
  var l = <int>[];
  l[0] ??= 2.5;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the ??= write-back asks the value formal, got {text:?}"
    );
    assert!(
        text[0].contains("'double'") && text[0].contains("'int'"),
        "the message spells the value and the element formal, got {text:?}"
    );
}

/// A matching `??=` stays silent: `s[1] ??= 3` over a `List<int?>` —
/// the `int` value fits the nullable element.
#[test]
fn if_null_index_write_matching_stays_silent() {
    let report = report_ops(
        "\
void g() {
  var s = <int?>[];
  s[1] ??= 3;
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").is_empty(),
        "the nullable element accepts, got {:?}",
        texts(&report, "invalid_assignment")
    );
}

/// Routed — the call bounds face asks the strict relation: `void`
/// against `num`/`Object`/an `int Function()` bound refuses with the
/// oracle's `doesn't conform` rows (upstream
/// `type_arguments_verifier.dart:373` asks `isSubtypeOf`; our verdict
/// site asks the assignability relaxation). Owning lane: invocation
/// (the call site's one-token rewire to [`Engine::slot_subtype`],
/// `assignability.rs` holds the strict twin).
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn void_type_argument_refuses_the_bounds() {
    let report = report_ops(
        "\
void f<T extends num>(T t) {}
void g<T extends Object>() {}
void h<T extends int Function()>() {}
void m() {
  f<void>(3);
  g<void>();
  h<void>();
}
",
    );
    let text = texts(&report, "type_argument_not_matching_bounds");
    assert_eq!(
        offsets(&report, "type_argument_not_matching_bounds").len(),
        3,
        "void refuses every written bound, got {text:?}"
    );
    assert!(
        text[0].contains("'void'") && text[0].contains("'num'"),
        "the first row spells the argument and the bound, got {text:?}"
    );
}

/// Routed — the `??` composite as a send argument joins its operands:
/// `f(a ?? 3)` under `void f(int x)` reads `int` (the join), so the
/// argument check stays silent. Today the argument face reads the bare
/// `int?` operand and reports. Owning lane: invocation (the argument
/// face's consult of the join-aware [`Engine::rhs_row_ty`], now
/// `pub(crate)`).
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn if_null_composite_send_argument_joins() {
    let report = report_ops(
        "\
void f(int x) {}
void m(int? a) {
  f(a ?? 3);
}
",
    );
    assert!(
        offsets(&report, "argument_type_not_assignable").is_empty(),
        "the joined argument fits the formal, got {:?}",
        texts(&report, "argument_type_not_assignable")
    );
}

/// Routed — S1c: the promoted-nullable bound must flatten `await x` to
/// the bare parameter (`X & X1?` strips to `X`), so the `Never?` sink
/// reports `'X' -> 'Never?'`. Owning face: the flow fold's
/// intersection-capable promoted face (flow lane, #3915) or the read
/// site's two-face consult — the pinned evidence in
/// `ticket_3893_tests.rs` carries the full derivation.
#[test]
#[ignore = "flow fold: the intersection X&X1? is not distinguishable at the read site (owner flow #3915, per ticket_3893 S1c evidence)"]
fn await_promoted_nullable_bound_falls_back_to_the_parameter() {
    let report = report_ops(
        "\
void f<X, X1 extends Future<Never>>(X x) async {
  if (x is X1?) {
    Never? a = await x;
  }
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the flattened bare parameter refuses Never?, got {text:?}"
    );
    assert!(
        text[0].contains("'X'") && text[0].contains("'Never?'"),
        "the message spells the bare parameter, got {text:?}"
    );
}

/// The `FutureOr<void>` source refuses the `Object` target: the Left
/// FutureOr conjunction asks `void <: Object` (false — a void source
/// subtypes only the top faces) and `Future<void> <: Object` (true), so
/// `Object o = h()` over `FutureOr<void> h()` reports the oracle's
/// `FutureOr<void>` -> `Object` row at the right-hand span.
#[test]
fn future_or_void_source_refuses_the_object_target() {
    let report = report_ops(
        "\
import 'dart:async';
FutureOr<void> h() => null;
void m() {
  Object o = h();
  use(o);
}
void use(Object o) {}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "the FutureOr source's void leg refuses Object, got {text:?}"
    );
    assert!(
        text[0].contains("'FutureOr<void>'") && text[0].contains("'Object'"),
        "the message spells the union source and the target, got {text:?}"
    );
}
