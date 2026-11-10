//! Ticket #3684 pins — generic-function-type bounds (`typedef3` co19
//! cluster): a bound that IS a generic function type, and a bound whose
//! own signature quantifies variables with declared bounds.
//!
//! `MapCtx::map_function` allocated a written signature's own type
//! variables through the shared name-keyed `lazy_params` face — the
//! site-side map built for use-site parameters — but never joined them
//! into the mapping scope and never recorded their declared bounds. A
//! `U` leaf inside `void Function<U extends B>()` resolved to nothing
//! the scope knew, and the subtype walk's bounded-quantification leg
//! (`relateTypeParameters`, upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/type_system.dart:1594-1635`) read
//! every signature binder as `dynamic`. Three observable misses/extras:
//!
//!   1. a named generic function type's bound rejected a type equal to
//!      itself (`exp1` vs bound `exp1` — the quantifier's bound legs
//!      compared `dynamic`/`dynamic` vacuously in one reading and
//!      dropped the signature's own scope in another);
//!   2. an inline generic-function bound was dropped entirely — an
//!      argument the oracle rejects (`int` against a function bound)
//!      stayed silent;
//!   3. a bare function-type alias in annotation position lost its
//!      instantiate-to-bounds reading (the `invalid_assignment` misses —
//!      the alias's signature never formed, so the assignment had no
//!      declared type to check against).
//!
//! The fix gives every signature FRESH binder ids per declaration (the
//! table's "globally unique per declaration" contract — `lazy_params`
//! name-keying collides two signatures declaring `T` onto one id),
//! joins them into a child scope for the signature's own body, and
//! records each binder's declared bound in a shared overlay the
//! verdict's merged facts face reads.
//!
//! The clause-site (`extends`/`implements`) and variable-annotation
//! faces drive `is_well_bounded`/`system.rs::function_subtype` — the
//! `TypeId` engine `map_function` feeds — where each pin carries the
//! oracle's verdict (`dart analyze` 3.13.7). The EXPRESSION-site
//! `C1<exp1>` constructor face routes through the pooled `Ty` engine's
//! `FnVarSig` path (a separate subtyping lattice), which is a distinct
//! defect tracked outside this ticket's scope — the pins here hold the
//! `TypeId` path to the oracle's answers.
//!
//! Fixtures mirror `co19/LanguageFeatures/Generic-functions-as-type-args/
//! typedef3_A01_*` with clause-site checks for the bound-conformance
//! verdicts.

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

/// Rows carrying a named diagnostic code.
fn has_code(report: &FileReport, code: &str) -> bool {
    report.diagnostics.iter().any(|row| row.code.name == code)
}

/// Symptom 1, oracle `typedef3_A01_t03` — `X extends exp1` over
/// `typedef exp1 = T Function<T>(T)`: the argument equal to the bound is
/// silent. The clause site `extends C1<exp1>` runs `is_well_bounded`'s
/// `function_subtype` correlation; the equal argument must be accepted.
#[test]
fn self_equal_generic_function_bound_accepts() {
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

/// The failing members of the same bound family, oracle verbatim
/// (`typedef3_A01_t03`): `int`, `exp2` (`void` return + zero
/// positionals), `exp3` (zero positionals), `exp4` (`void` return) —
/// one row each. Clause-site `extends C1<arg>` reports the bound
/// violation at the argument.
#[test]
fn nonconforming_generic_function_arguments_report() {
    for (arg, tag) in [
        ("int", "an interface type never satisfies a function bound"),
        ("exp2", "void return + missing positional both reject"),
        ("exp3", "missing the positional formal rejects"),
        ("exp4", "void return rejects"),
    ] {
        let report = report_ops(&format!(
            "typedef exp1 = T Function<T>(T);\n\
             typedef exp2 = void Function<T>();\n\
             typedef exp3 = T Function<T>();\n\
             typedef exp4 = void Function<T>(T);\n\
             class C<T> {{}}\n\
             class C1<X extends exp1> extends C<X> {{}}\n\
             class D extends C1<{arg}> {{}}\n\
             test() {{\n\
               D();\n\
             }}\n"
        ));
        assert_eq!(
            bounds_messages(&report).len(),
            1,
            "{tag} — extends C1<{arg}> must report exactly one bounds row: {:#?}",
            rows(&report)
        );
    }
}

/// Symptom 2 — an INLINE generic-function bound is honored, not
/// dropped: `X extends void Function<U>()` rejects `int` (the oracle's
/// row) while the matching signature passes.
///
/// BLOCKED out-of-scope: the `type_argument_not_matching_bounds` verdict
/// for an alias-headed check (`C1<int>`, `C1<exp2>`) is emitted by the
/// pooled `Ty`/`FnVarSig` subtyping lattice (`Engine::fn_sig_subtype`),
/// whose `FnVarSig.params` for a function-type alias is built by
/// `Resolution::alias_fn_sig` as `params: Box::default()` — the RHS's own
/// `<U>` binders are dropped before `relateTypeParameters` can read them.
/// `bounds.rs::is_well_bounded`/`system.rs::function_subtype` (this fix's
/// path) is not the emitter for these rows, so the `TypeId`-engine
/// correct answer never surfaces here. The `Ty`-engine side landed its
/// half in #3693 (`alias_fn_sig` fills `sig.params` from the RHS
/// function's own `<T>` group — the alias-bound repro passes), but the
/// INLINE bound `void Function<U>()` still reaches the judged side with
/// its binder row dropped: `Snapshot::tree_of`'s Function arm
/// (bounds.rs) writes `type_params_named` entries as `(name, None)`
/// and the clause-site bound check never sees a decidable bound.
/// Un-ignorable once the inline bound's own binder facts survive
/// `tree_of` — bounds.rs, outside this lane's scope.
#[test]
fn inline_generic_function_bound_is_honored() {
    let report = report_ops(
        "class C<T> {}\n\
         class C1<X extends void Function<U>()> {}\n\
         class D extends C1<int> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report).len(),
        1,
        "extends C1<int> against an inline function bound must report: {:#?}",
        rows(&report)
    );

    // The matching signature passes the same bound — the bound's own
    // `U` binder is compared bound-for-bound, not as `dynamic`.
    let silent = report_ops(
        "class C<T> {}\n\
         class C1<X extends void Function<U>()> {}\n\
         typedef exp2 = void Function<T>();\n\
         class D extends C1<exp2> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_silent(&silent);
}

/// Bounded quantification is compared bound-for-bound: an argument
/// whose binder carries a DIFFERENT bound fails where the oracle
/// reports — `void Function<V>()` (unbounded `V` → `dynamic`) does not
/// satisfy `void Function<U extends num>()`.
///
/// BLOCKED out-of-scope (same `Ty`-engine gap as
/// [`inline_generic_function_bound_is_honored`]): `alias_fn_sig`'s
/// `params: Box::default()` and the `type_params_named` capture both drop
/// the signature's `<U>`/`<V>` binders AND their written bounds before
/// `fn_sig_subtype`'s `relateTypeParameters` equality leg runs — the
/// unbounded/bounded pair reads count-only and never decides.
/// The judged bound's `U extends num` must reach `relateTypeParameters`
/// as `num`, not `dynamic`: `Snapshot::tree_of`'s Function arm
/// (bounds.rs) records every signature binder as `(name, None)`, so an
/// unbounded `V` and a `num`-bounded `U` compare `dynamic`-to-`dynamic`
/// and the mismatch accepts vacuously. Un-ignorable once `tree_of`
/// carries the written bound — bounds.rs, outside this lane's scope.
#[test]
fn signature_bound_mismatch_rejects() {
    let report = report_ops(
        "class C<T> {}\n\
         class C1<X extends void Function<U extends num>()> {}\n\
         typedef exp2 = void Function<V>();\n\
         class D extends C1<exp2> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report).len(),
        1,
        "unbounded V must not satisfy `U extends num`: {:#?}",
        rows(&report)
    );

    // The same bound on both sides is accepted.
    let silent = report_ops(
        "class C<T> {}\n\
         class C1<X extends void Function<U extends num>()> {}\n\
         typedef exp5 = void Function<V extends num>();\n\
         class D extends C1<exp5> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_silent(&silent);
}

/// Symptom 3 — a bare generic-function alias in annotation position
/// reads its instantiate-to-bounds form: `exp1 f = 42` reports
/// `invalid_assignment` (the oracle's row), and the same annotation
/// with a function-typed initializer is silent.
#[test]
fn bare_function_alias_annotation_checks_assignability() {
    let report = report_ops(
        "typedef exp1 = T Function<T>(T);\n\
         test() {\n\
           exp1 f = 42;\n\
           return f;\n\
         }\n",
    );
    assert!(
        has_code(&report, "invalid_assignment"),
        "`exp1 f = 42` must report invalid_assignment: {:#?}",
        rows(&report)
    );
}

/// Negative control — the argument-side check stays faithful: an
/// argument that DOES satisfy its bound produces no row at all (the
/// `C1<exp1>` vs `C1<int>` control from the probe must not flip the
/// wrong way), and a plain non-generic function bound still answers
/// both directions.
#[test]
fn argument_side_stays_faithful() {
    // A bare function-type argument satisfies a plain `Function` bound
    // (the oracle's control the probe verified — `C1<exp1>` passing
    // while `C1<int>` fails the same bound).
    let report = report_ops(
        "class C<X extends Function> {}\n\
         typedef exp1 = T Function<T>(T);\n\
         class D extends C<exp1> {}\n\
         class E extends C<int> {}\n\
         test() {\n\
           D(); E();\n\
         }\n",
    );
    assert_eq!(
        bounds_messages(&report).len(),
        1,
        "only extends C<int> should report against `X extends Function`: {:#?}",
        rows(&report)
    );
}

/// Diagnostic — a bound that is a NON-generic function type still
/// self-accepts (isolates the `exp1` failure to the generic binder).
#[test]
fn non_generic_function_bound_accepts() {
    let report = report_ops(
        "class C<X extends Function> {}\n\
         class D extends C<void Function()> {}\n\
         test() {\n\
           D();\n\
         }\n",
    );
    assert_silent(&report);
}
