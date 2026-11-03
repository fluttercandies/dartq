//! #3692 pins — a function-type bound on the callee's own type variable
//! must reach the invocation-site bounds verdict: `f<F>()` where
//! `f<T extends void Function<U extends B>()>` and `F = void Function<T
//! extends A>()` reports `type_argument_not_matching_bounds` — the
//! quantifier bound decides through the signature-binder facts (#3686's
//! `relateTypeParameters` leg) once the judged bound is actually captured.
//!
//! Probe evidence (this lane's env-gated stderr dumps during the run):
//!
//! * `exec_bounds["f"]` is EMPTY for every `T extends ...Function(...)`
//!   bound — a generic-function-type's own `()` formal list fires
//!   `begin_formal_parameters` at `formal_list_depth == 0` while the
//!   enclosing `<T ...>` group is still open, clearing `pending_exec_name`
//!   before `end_type_variables` runs `pair_exec_window` (the `#1314
//!   FAM10` clear in `begin_formal_parameters`).
//! * the bound TypeTree degrades `void Function<U extends B>()` to
//!   `void Function()`: the nested `<U>` group's window parks in
//!   `param_windows` while the enclosing `bound_sink`'s open function
//!   level never learns the signature's own variables (`Bounds::
//!   end_type_variables`'s nested branch).
//! * `top_level_decl("f")` answers `type_param_names = [U]` — the
//!   collector's flat `type_var_group`/`type_var_bounds` buffers are
//!   cleared by the nested `<U>` group and re-attach the signature's own
//!   variable as `f`'s (the element-model half of this defect, reported
//!   with the capture diffs).
//!
//! The two repro pins stay `#[ignore]`d until the upstream capture diffs
//! named above land (this lane's scope is `invocation.rs`; the capture
//! points live outside it).
//!
//! Oracle ground truth (upstream Dart SDK source,
//! `pkg/analyzer/test/src/diagnostics/
//! type_argument_not_matching_bounds_test.dart`
//! `test_methodInvocation_genericFunctionTypeArgument_mismatch#0`):
//! `'F' doesn't conform to the bound 'void Function<U extends B>()' of the
//! type parameter 'T'.` anchored at the written argument `F`
//! (`lib/test.dart|7:5|1` in the conformance corpus — the rig's own line
//! offsets shift under the prelude).

use super::test_rig::{assert_silent, report_ops, rows};

/// The oracle-verbatim mismatch case: a `void Function<T extends A>()`
/// argument against the `void Function<U extends B>()` bound.
///
/// Oracle (`TypeArgumentNotMatchingBoundsTest/
/// test_methodInvocation_genericFunctionTypeArgument_mismatch#0`): one row,
/// `type_argument_not_matching_bounds` at the `F` argument.
#[test]
#[ignore = "blocked on the upstream bound-capture diffs this lane reports; \
            probe: exec_bounds[\"f\"] comes back empty"]
fn gft_bound_mismatch_reports() {
    let report = report_ops(
        "class A {}\n\
         class B {}\n\
         typedef F = void Function<T extends A>();\n\
         void f<T extends void Function<U extends B>()>() {}\n\
         void g() {\n\
           f<F>();\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("type_argument_not_matching_bounds")),
        "expected type_argument_not_matching_bounds, got {names:?}"
    );
}

/// Same-family plain function-type bound: `T extends void Function()`
/// misses identically — the signature's own `()` clears the pairing latch
/// even with no nested `<...>` group (`int` is no function type, so the
/// oracle reports).
#[test]
#[ignore = "blocked on the same latch-clear diff as \
            gft_bound_mismatch_reports; probe: exec_bounds[\"f\"] comes back \
            empty"]
fn plain_fn_bound_mismatch_reports() {
    let report = report_ops(
        "void f<T extends void Function()>() {}\n\
         void g() {\n\
           f<int>();\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("type_argument_not_matching_bounds")),
        "expected type_argument_not_matching_bounds, got {names:?}"
    );
}

/// The matching-bound twin stays silent — the same signature with `num`
/// on both sides (`test_methodInvocation_genericFunctionTypeArgument_
/// match#0` spells `num`; the rig's prelude declares it). Silent today for
/// the wrong reason (the judged bound never lands); it must STAY silent
/// through the real `relateTypeParameters` bound-equality leg once the
/// capture lands — this pin guards both directions.
#[test]
fn gft_bound_match_is_silent() {
    let report = report_ops(
        "typedef F = void Function<T extends num>();\n\
         void f<T extends void Function<X extends num>()>() {}\n\
         void g() {\n\
           f<F>();\n\
         }\n",
    );
    assert_silent(&report);
}

/// Negative control: a non-generic bound mismatch through the same
/// verdict still reports — the capture fix must not widen the silent face
/// (`T extends num` bound, `String` argument).
#[test]
fn non_generic_bound_mismatch_still_reports() {
    let report = report_ops(
        "void f<T extends num>() {}\n\
         void g() {\n\
           f<String>();\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("type_argument_not_matching_bounds")),
        "expected type_argument_not_matching_bounds, got {names:?}"
    );
}
