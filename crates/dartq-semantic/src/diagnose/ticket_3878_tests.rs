//! #3878 seam pins — the operator-face extraction (E-op + E13):
//!
//! The census families this ticket extracted diverge through three
//! mechanisms, each rooted OUTSIDE the two files this lane was granted
//! (the granted `resolved_ops.rs` name resolves to the flow TypeOps
//! back-end at `src/resolved_ops.rs`, which hosts none of them; the
//! operator faces live in `engine_core.rs`/`assignability.rs` — routed
//! to lane D per the brief's own rule — and `row_resolution.rs`, a live
//! file this lane must not write). So this file is the seam the routed
//! fixes land against: the controls below are green today and guard the
//! exact faces the routed work will touch; the ignored pins are the
//! acceptance suite for each routed fix (they fail while the bug stands,
//! pass when the owning lane lands, and are `#[ignore]`-gated so the
//! suite stays green until then).
//!
//! Routed record (every row accounted, zero silent drops):
//!
//! * E-op argument-context ladder (14 rows, number_operator_context_test)
//!   — `invoke_downward_context`/`send_position_context` reads the
//!   enclosing generic call's formal RAW (`send_formals` in
//!   `row_resolution.rs`), so `context<int>(…)`'s written `int` never
//!   instantiates the formal and the numeric context ladder degrades to
//!   `num`; `refine_numeric_context`'s clamp arm also shares the
//!   remainder arm's NEGATIVE receiver gate (`!T <: double?`) where the
//!   upstream clamp arm is POSITIVE (`T <: double?`,
//!   `_refineNumericInvocationContextNullSafe` clamp ladder, Dart SDK
//!   source `pkg/analyzer/lib/src/dart/element/type_system.dart`
//!   :1949-1962). Owning file: `row_resolution.rs` (row-model lane).
//! * E-op dynamic-receiver const operand (6 rows, double_in_int_op id1-6)
//!   — the `|` argument check on a const-`dynamic` receiver reads the
//!   operand's CONST VALUE type (`double`) where the oracle checks the
//!   static type (`dynamic`, no check). Owning file: the dynamic/const
//!   operator-argument faces (lane B3 / const faces).
//! * E13 numeric-ladder Never guard (12 rows, number_operator_typing_test)
//!   — the ladder's `S <: double?` arm misses the upstream
//!   `!s.isBottom` guard (`type_system.dart:2098`), so `T op never`
//!   answers `double` where the oracle answers `num`. Owning file:
//!   `engine_core.rs` (`refine_numeric_binary`, lane D).
//! * E13 compound-result row face (34 rows, typing_test) — the compound
//!   assignment's value row keeps the RHS type (`d += i1` reads `int`,
//!   `d += never` reads `Never`) where upstream's
//!   `AssignmentExpressionImpl.staticType` is the operator RESULT the
//!   write-back verdict already computes; the compound's receiver read
//!   also skips the flow promotion (`d -= 1` on `Object&double`
//!   resolves on `Object`, 11 undefined_operator rows). Owning file:
//!   `assignability.rs` (`check_compound_assignment`, lane D).
//! * E13 promoted binary operand (7 rows, typing_test) — the binary
//!   fold's operand faces read unpromoted declared types (`i + ni` over
//!   `ni` promoted `N&int` answers `num`). Owning file:
//!   `engine_core.rs` (`fold_binary` operand reads, lane D).
//! * E13 null-assert on `Null` (2 rows, test_nullCheck_nullableContext)
//!   — `non_null_form` has no `Null → Never` arm (upstream
//!   `promoteToNonNull`, `type_system.dart:1479`). Owning files:
//!   `receiver_face.rs`/`types.rs`.
//! * E13 conditional branch promotion (2 rows, named_parameters_test
//!   50:41/54:41) — the conditional fold reads arm rows without the
//!   flow promotion the test applied (`c == null ? 0 : c` joins `int?`
//!   where the oracle joins `int`). Owning file: `engine_core.rs`
//!   (`fold_conditional`, lane D).
//! * E13 field-initializer numeric context (1 row, typing_test 1090:14)
//!   — a `double`-typed field's initializer context never reaches the
//!   binary refinement (`n + 1` stays `num`). Owning file:
//!   `engine_core.rs`/const faces (lane D).
//!
//! Oracle-only residuals beside these (not extras, recorded for the
//! owning lanes): the corpus's `unused_local_variable` on a
//! compound-only-written local, the `undefined_named_parameter` rows
//! (invocation named-pairing), and the Never-flow `dead_code` +
//! `unnecessary_type_check` pair (flow dead-code face).

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

// —— controls (green today; guard the faces the routed work touches) ——

/// The refinement's `S <: double?` arm answers `double` for an `int`
/// receiver with a `double` operand (`int + double` stays `double`).
#[test]
fn control_refinement_s_double_arm_holds() {
    let report = report_ops(
        "\
void f(int i, double d) {
  double x = i + d;
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").is_empty(),
        "int + double must read double, got {:?}",
        texts(&report, "invalid_assignment")
    );
}

/// The same refinement feeds the sink verdict: `int + double` into an
/// `int` sink reports exactly the `double -> int` mismatch.
#[test]
fn control_refinement_result_reaches_the_sink() {
    let report = report_ops(
        "\
void f(int i, double d) {
  int y = i + d;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "expected exactly the double -> int mismatch, got {text:?}"
    );
    assert!(
        text[0].contains("'double'") && text[0].contains("'int'"),
        "expected double -> int, got {text:?}"
    );
}

/// The relational operators' result is `bool` — clean into a `bool`
/// sink, refused into an `int` sink.
#[test]
fn control_relational_result_is_bool() {
    let report = report_ops(
        "\
void f() {
  bool b = 1 < 2;
  int z = 1 < 2;
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").len() == 1,
        "expected exactly the bool -> int mismatch, got {:?}",
        texts(&report, "invalid_assignment")
    );
    assert!(
        offsets(&report, "non_bool_condition").is_empty(),
        "relational results must not feed non_bool faces"
    );
}

/// A null-assert promotes a nullable face to its non-null form
/// (`Object?` reads `Object`), so the `int` sink reports `Object`.
#[test]
fn control_null_assert_promotes_nullable() {
    let report = report_ops(
        "\
void k(Object? o) {
  int i = o!;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "expected exactly the Object -> int mismatch, got {text:?}"
    );
    assert!(
        text[0].contains("'Object'") && !text[0].contains("Object?"),
        "the asserted face must be non-null, got {text:?}"
    );
}

/// The compound write-back verdict is green today (`d += i1` writes a
/// `double` back over a `double` target) — the row-face fix must not
/// disturb the verdict this control locks.
#[test]
fn control_compound_write_back_verdict_stays_green() {
    let report = report_ops(
        "\
void m() {
  double d = 1.0;
  int i1 = 2;
  d += i1;
  double u = d;
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").is_empty(),
        "compound write-back must stay silent, got {:?}",
        texts(&report, "invalid_assignment")
    );
}

// —— routed acceptance pins (fail-before, pass-after; #[ignore]-gated
// —— until the owning lane lands the fix named in the reason) ———--------

/// `int + never` answers `num` (the upstream ladder skips a bottom `S`
/// on every arm), so the `double` sink refuses it. Lane D,
/// `engine_core.rs` `refine_numeric_binary`: the `S <: double?` arm
/// needs the upstream `!s.isBottom` guard (Dart SDK source,
/// `pkg/analyzer/lib/src/dart/element/type_system.dart:2098`).
#[test]
fn never_operand_keeps_the_ladder_at_num() {
    let report = report_ops(
        "\
void f() {
  int i = 1;
  Never never = throw '';
  double r = i + never;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "i + never is num; the double sink must refuse it, got {text:?}"
    );
    assert!(
        text[0].contains("'num'"),
        "the operand face must be num, not double, got {text:?}"
    );
}

/// The compound assignment's VALUE is the operator result, not the
/// RHS: `var x = d += i1` reads `double`, so the `int` sink refuses.
/// The `assignability.rs` half landed (`check_compound_assignment`
/// retypes the expression's tail row to the computed result face); the
/// end-to-end still needs the listener half — `end_variable_initializer`
/// has no compound-assignment arm, so `var x = d += …` never opens the
/// inference window and `x` stays untyped. Owning file: `listener.rs`
/// (the var-init window lane).
#[test]
#[ignore = "listener.rs: end_variable_initializer lacks the compound-assignment window arm (assignability.rs half landed)"]
fn compound_result_row_carries_the_operator_result() {
    let report = report_ops(
        "\
void f() {
  double d = 1.0;
  int i1 = 2;
  var x = d += i1;
  int w = x;
}
",
    );
    let text = texts(&report, "invalid_assignment");
    assert_eq!(
        offsets(&report, "invalid_assignment").len(),
        1,
        "x is double; the int sink must refuse it, got {text:?}"
    );
    assert!(
        text[0].contains("'double'"),
        "the compound's value face must be double, got {text:?}"
    );
}

/// The compound's receiver read follows the flow promotion
/// (`Object&double` resolves `-=` on `double`). The `assignability.rs`
/// half landed (`check_compound_assignment` asks
/// `read_type_at` for the promoted receiver), and the pass-2
/// operator-face verdict reads the promoted face through the same
/// latch — on the rig's member-less `double` stub the walk honestly
/// misses, but the reported row names `'double'` (the promoted face),
/// never `'Object'` (the declared face).
#[test]
fn compound_read_uses_the_promoted_receiver() {
    let report = report_ops(
        "\
void f(Object d) {
  if (d is! double) return;
  d -= 1;
}
",
    );
    let rows = texts(&report, "undefined_operator");
    assert_eq!(rows.len(), 1, "one operator row on the stub face: {rows:?}");
    assert!(
        rows[0].contains("'double'") && !rows[0].contains("'Object'"),
        "the read face is the promoted double, not the declared Object: {rows:?}"
    );
}

/// The conditional's else branch reads the test's promotion
/// (`c == null ? 0 : c` joins `int`, not `int?`). The fold consults
/// landed in `engine_core.rs` `fold_conditional`, but the listener
/// folds run pass 1 while `read_types` fills after it — every
/// latch consult inside a fold sees an empty latch until the consult
/// moves to a pass-2 face. Owning face: the pass-phase fix (lane AD,
/// #3897).
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn conditional_else_branch_reads_the_promotion() {
    let report = report_ops(
        "\
void f(int? c) {
  int y = c == null ? 0 : c;
}
",
    );
    assert!(
        offsets(&report, "invalid_assignment").is_empty(),
        "the else branch promotes c to int, got {:?}",
        texts(&report, "invalid_assignment")
    );
}

/// `f(null)!` — the null-assert over a `Null` face answers `Never`
/// (upstream `promoteToNonNull`'s `isDartCoreNull` arm, Dart SDK
/// source `pkg/analyzer/lib/src/dart/element/type_system.dart:1479`),
/// and `Never` returns into `int` clean. Owning files:
/// `receiver_face.rs` `null_asserted_after` + `types.rs`
/// `non_null_form` (row-model lane).
#[test]
#[ignore = "row-model lane: non_null_form lacks the Null -> Never arm (2 census rows)"]
fn null_assert_over_null_types_never() {
    let report = report_ops(
        "\
T f<T>(T t) => t;
int g() => f(null)!;
",
    );
    assert!(
        offsets(&report, "return_of_invalid_type").is_empty()
            && offsets(&report, "null_check_always_fails").is_empty(),
        "Never returns clean and the check is not vacuous here, got return={:?} check={:?}",
        texts(&report, "return_of_invalid_type"),
        texts(&report, "null_check_always_fails")
    );
}
