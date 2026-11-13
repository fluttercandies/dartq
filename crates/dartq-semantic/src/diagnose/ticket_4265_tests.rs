//! #4265 pins: the false-positive families the 0.1.23 engine face added
//! on cockpit-cas-b (each emitted nothing on 0.1.21 and is absent from
//! the dart analyze 3.13.7 oracle).
//!
//! - (1) a composite-result initializer (`final x = c ? a : b`, `??`,
//!   binary folds) left its operand rows live under the `RowShape::Value`
//!   result row, so `end_variable_initializer` never matched the window
//!   and the inferred local stayed `UNKNOWN_NULLABLE` — downstream
//!   `supported = x` wrote `Unknown`, the `bool?` local never promoted,
//!   and `!x`/`x` checks reported `unchecked_use_of_nullable_value`.
//!   The composite-result arm binds the last `RowShape::Value` row's
//!   type to the inferred local, matching the oracle's
//!   `_inferLocalVariable`.
//! - (2) `Future<void>(() async { ... })` without `dart:async` import:
//!   `core_unindexed_future_formals` synthesizes `FutureOr<T>
//!   Function()` for the `Future<T>` ctor's computation formal. The
//!   synthetic `FutureOr` head then tripped `future_value_type`'s
//!   `named_type_is_type("FutureOr")` scope gate (written unimported
//!   `FutureOr` resolves to `InvalidType` upstream), collapsing to
//!   `Object?` and reporting `body_might_complete_normally_nullable`
//!   'FutureOr<Object?>'. Closure-context rows now carry a
//!   `synthetic_ctx` flag that bypasses the scope gate — written
//!   unimported `FutureOr` (ticket_3132 pin) still reports.

use super::test_rig::{code_names, report_ops};

/// (1) The ternary-inferred local: `final probe = cond ? a : b` binds
/// the joined type, `probe.supported` reads `bool`, the `bool?`
/// assignment promotes, and `!flag` stays silent (oracle silent).
#[test]
fn ternary_inferred_local_promotes_through_member_read() {
    let report = report_ops(
        "class ProbeResult {\n\
           final bool supported;\n\
           const ProbeResult({required this.supported});\n\
         }\n\
         ProbeResult probe() => const ProbeResult(supported: true);\n\
         void main() {\n\
           bool? flag;\n\
           final cond = true;\n\
           final result = cond\n\
               ? probe()\n\
               : const ProbeResult(supported: false);\n\
           flag = result.supported;\n\
           if (!flag) {\n\
             throw 'fail';\n\
           }\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&report)
    );
}

/// (1) The `&&` junction variant: `flag && !unattended` where
/// `unattended` was assigned from a member read of a ternary-inferred
/// local — the promotion chain must reach the condition operand
/// (oracle silent).
#[test]
fn ternary_inferred_local_promotes_through_and_operand() {
    let report = report_ops(
        "class Support {\n\
           final bool supported = true;\n\
         }\n\
         bool pick(Support s, bool flag) {\n\
           bool? unattended;\n\
           final probe = flag\n\
               ? s\n\
               : const Support();\n\
           unattended = probe.supported;\n\
           if (flag && !unattended) {\n\
             return false;\n\
           }\n\
           return true;\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&report)
    );
}

/// (1) The genuine-nullable case still reports: `bool?` never assigned
/// before `!unattended` is a real `unchecked_use_of_nullable_value`
/// (oracle reports both the `&&` operand and the bare condition).
#[test]
fn unassigned_nullable_condition_still_reports() {
    let report = report_ops(
        "bool pick(bool flag) {\n\
           bool? unattended;\n\
           if (flag && !unattended) {\n\
             return false;\n\
           }\n\
           return true;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report)
            .iter()
            .filter(|&&c| c == "unchecked_use_of_nullable_value")
            .count(),
        2,
        "{:?}",
        code_names(&report)
    );
}

/// (2) The synthetic-FutureOr closure context: `Future<void>(() async
/// { ... })` without `dart:async` — the synthetic `FutureOr<void>`
/// formal must not trip the scope gate (oracle silent).
#[test]
fn synthetic_futureor_closure_context_stays_silent() {
    let report = report_ops(
        "class FutureOr<T> {}\n\
         class Future<R> { Future(FutureOr<R> Function() computation) {} }\n\
         void run() {\n\
           Future<void>(() async {\n\
             return;\n\
           });\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// (2) The written-unimported-FutureOr pin (ticket_3132): the existing
/// `ticket_3132_tests::async_unimported_futureor_reports_nullable_object`
/// covers the declared path — this test verifies the synthetic bypass
/// does not leak: a CLOSURE context carrying a written `FutureOr`
/// (in-scope, declared locally) still unwraps correctly through
/// `future_value_type`.
#[test]
fn closure_context_futureor_unwraps_when_in_scope() {
    let report = report_ops(
        "class FutureOr<T> {}\n\
         class Future<R> { Future(FutureOr<R> Function() computation) {} }\n\
         void run() {\n\
           Future<FutureOr<int>>(() async {\n\
             return 42;\n\
           });\n\
         }\n",
    );
    // `FutureOr<int>` is in scope (declared locally): the synthetic
    // `FutureOr<FutureOr<int>>` context must unwrap to `FutureOr<int>`,
    // and `return 42` must be assignable to `FutureOr<int>` — no
    // `return_of_invalid_type_from_closure`.
    assert!(
        !code_names(&report).contains(&"return_of_invalid_type_from_closure"),
        "{:?}",
        code_names(&report)
    );
}

/// (2) The `Future<int>` variant: the synthetic `FutureOr<int>`
/// context must produce `Future<int>` (not `Future<void>`), so a
/// `return 42;` inside the closure is assignable and no
/// `return_of_invalid_type_from_closure` fires (oracle silent).
#[test]
fn synthetic_futureor_int_context_accepts_int_return() {
    let report = report_ops(
        "class FutureOr<T> {}\n\
         class Future<R> { Future(FutureOr<R> Function() computation) {} }\n\
         void run() {\n\
           Future<int>(() async {\n\
             return 42;\n\
           });\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"return_of_invalid_type_from_closure"),
        "{:?}",
        code_names(&report)
    );
}

/// (2) The `Future<int>` variant with a wrong return type still
/// reports `return_of_invalid_type_from_closure` — the synthetic
/// context must not suppress genuine mismatches.
#[test]
fn synthetic_futureor_int_context_rejects_wrong_return() {
    let report = report_ops(
        "class FutureOr<T> {}\n\
         class Future<R> { Future(FutureOr<R> Function() computation) {} }\n\
         void run() {\n\
           Future<int>(() async {\n\
             return 'not an int';\n\
           });\n\
         }\n",
    );
    assert!(
        code_names(&report).contains(&"return_of_invalid_type_from_closure"),
        "{:?}",
        code_names(&report)
    );
}

/// (1) The is-test ternary variant: `final m = x is T ? x : null` binds
/// `T?` through the composite fold's promoted arm — the parse-time LUB
/// starved `read_type_at` (the read-type table fills after `parse_unit`),
/// so the inferred local held `Object?` and `m['k']` reported
/// `undefined_operator`. The pass-2 composite re-bind re-types it `T?`
/// (oracle silent — the cockpit-cas-b `schema is Map ? schema : null`
/// shape).
#[test]
fn is_test_ternary_inferred_local_promotes_through_index() {
    let report = report_ops(
        "void f(Object? schema) {\n\
           final schemaMap = schema is Map ? schema : null;\n\
           if (schemaMap != null) {\n\
             schemaMap['properties'];\n\
           }\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_operator"),
        "{:?}",
        code_names(&report)
    );
}
