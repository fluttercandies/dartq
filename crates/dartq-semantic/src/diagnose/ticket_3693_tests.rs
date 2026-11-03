//! Ticket #3693 pins — the pooled `Ty`/`FnVarSig` subtyping lattice must
//! carry a function-type alias's OWN signature binders (the RHS `<T>` of
//! `typedef F = R Function<T>(...)`) into `FnVarSig.params`, so
//! `relateTypeParameters` reads the written binders' declared bounds
//! instead of an empty parameter list.
//!
//! # The defect this ticket fixes
//!
//! `Resolution::alias_fn_sig` destructured `alias_fn_shape`'s six-tuple and
//! dropped `rhs_type_params` — the RHS function's own `<T>` binder list —
//! then emitted `sig.params = Box::default()`. A bound or argument spelled
//! as a generic-function alias (`exp2 = void Function<T>()`) reached
//! `fn_sig_subtype`'s `relateTypeParameters` leg with arity `0`, and the
//! count gate mis-decided every subtype question against a bound that
//! carried its own `<U>` quantifier.
//!
//! The fix keeps the alias's own binders out of `sig.params` (they are the
//! declaring parameter list, not the signature's) and fills `params` from
//! the RHS function's own binders, substituting each binder's written
//! bound through the alias's use-site argument map — the same face
//! `instantiated_alias_sig` already answers.
//!
//! Oracle provenance: `dart analyze` 3.13.7 machine format on the fixtures
//! below (`type_argument_not_matching_bounds` at the written argument /
//! clause site).

use super::FileReport;
use super::test_rig::{assert_silent, report_ops, rows};

/// The `type_argument_not_matching_bounds` messages of one report, in
/// report order.
fn bounds_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "type_argument_not_matching_bounds")
        .map(|row| row.message.to_string())
        .collect()
}

/// The ticket's minimal repro: `exp1 = T Function<T>(T)` as a bound — an
/// argument whose own signature has zero positionals and a `void` return
/// fails the bound where the oracle reports. The alias's own `<T>` binder
/// must reach `relateTypeParameters` for the signature shape to compare
/// (the `0`-positional `exp2` never satisfies `T Function<T>(T)`).
#[test]
fn alias_rhs_binder_reaches_the_subtype_verdict() {
    let report = report_ops(
        "typedef exp1 = T Function<T>(T);\n\
         typedef exp2 = void Function<T>();\n\
         class C<T> {}\n\
         class C1<X extends exp1> extends C<X> {}\n\
         class D extends C1<exp2> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report).len(),
        1,
        "exp2 (`void Function<T>()`) must not satisfy `exp1` (`T Function<T>(T)`): {:#?}",
        rows(&report)
    );
}

/// The negative twin: an argument whose signature DOES satisfy `exp1` —
/// `exp1` itself — stays silent through the same binder-carrying path.
/// Guard against a fix that reports every alias argument.
#[test]
fn self_equal_alias_bound_stays_silent() {
    let report = report_ops(
        "typedef exp1 = T Function<T>(T);\n\
         class C<T> {}\n\
         class C1<X extends exp1> extends C<X> {}\n\
         class D extends C1<exp1> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_silent(&report);
}

/// A concrete non-function argument (`int`) still rejects against a
/// generic-function bound — the arity-0 regression must not silence the
/// shape mismatch.
#[test]
fn non_function_argument_still_rejects() {
    let report = report_ops(
        "typedef exp1 = T Function<T>(T);\n\
         class C<T> {}\n\
         class C1<X extends exp1> extends C<X> {}\n\
         class D extends C1<int> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report).len(),
        1,
        "int must not satisfy `exp1`: {:#?}",
        rows(&report)
    );
}

/// An inline (non-aliased) generic-function bound rejects a
/// `void Function<T>()` argument whose `void` return and missing
/// positional both fail the bound — the same signature-binder read from
/// the bound side's inline spelling.
///
/// BLOCKED out-of-scope (same drop as ticket_3684's pair): the judged
/// bound's inline `num Function<U>(num)` never reaches
/// `check_call_type_arg_bounds` as a function type — `Snapshot::tree_of`'s
/// Function arm (bounds.rs) drops the signature's own binder row, so the
/// bound collapses before `assignable` can reject the argument.
#[ignore = "inline bound collapses at Snapshot::tree_of (bounds.rs) — outside this lane's scope"]
#[test]
fn inline_generic_bound_rejects_mismatched_alias() {
    let report = report_ops(
        "class C<T> {}\n\
         class C1<X extends num Function<U>(num)> {}\n\
         typedef exp2 = void Function<T>();\n\
         class D extends C1<exp2> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report).len(),
        1,
        "exp2's void return and missing positional must fail `num Function<U>(num)`: {:#?}",
        rows(&report)
    );
}
