//! #3741 — `conflicting_generic_interfaces`, census
//! `census-20261103-133158.report.tsv`: the extras all ride two shapes the
//! oracle resolves differently than the name-keyed mirror did.
//!
//! ```text
//! class X extends A<double> with M {}   // mixin M on A<int>
//!   silent — an invalid application contributes NOTHING to the merge:
//!   `element.mixins` never reaches the hierarchy when any `on`
//!   constraint the supertype (or an earlier mixin) cannot satisfy —
//!   `mixin_application_not_implemented_interface` reports instead.
//!   Probed 3.13.7: even a satisfied sibling mixin in the same clause
//!   contributes nothing, and an invalid mixin's `implements` faces do
//!   not propagate either.
//! class A<T> implements B<List<T>> {}
//! class B<T> implements A<List<T>> {}   // a hierarchy loop
//!   silent — `_getHierarchy` pre-seeds the empty `_Hierarchy` before
//!   walking, so the cyclic re-entry collects no interfaces (the official
//!   reports `recursive_interface_inheritance` instead).
//! ```
//!
//! The inference bound fallback substitutes through the partial solution:
//! `M6<S, T extends S>` applied to `I<int>` lands `M6<int, int>` — the
//! oracle's `chooseFinalTypes` resolves `T`'s bound `S` to `int`, not a
//! bare `S`.

use crate::FileReport;
use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// Every `conflicting_generic_interfaces` message, in report order.
fn conflict_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "conflicting_generic_interfaces")
        .map(|d| d.message.to_string())
        .collect()
}

/// The whole with-clause drops when the superclass cannot satisfy the
/// mixin's `on` constraint — `extends I` reads `I<dynamic>`, which does
/// not implement `I<int>` (`mixin_declaration_inference_invalid_00`'s
/// A00 shape; the oracle answers `mixin_application_not_implemented_
/// interface`, never this code).
#[test]
fn unsatisfied_on_constraint_silences_the_whole_with_clause() {
    let report = report_ops(
        "class I<X> {} \
         mixin M0<T> on I<T> {} \
         class A00 extends I with M0<int> {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// The named mixin application twin: `class X = A<double> with M` —
/// `A<double>` does not implement `A<int>`
/// (`MixinApplicationNotImplementedInterfaceTest/test_classTypeAlias_
/// generic`).
#[test]
fn class_type_alias_invalid_application_stays_silent() {
    let report = report_ops(
        "class A<T> {} \
         mixin M on A<int> {} \
         class X = A<double> with M;\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// An invalid mixin's `implements` faces do not propagate either
/// (probed 3.13.7: `B<int>` and `B<String>` never reach the merge).
#[test]
fn invalid_mixin_interfaces_do_not_propagate() {
    let report = report_ops(
        "class A<T> {} \
         class B<T> {} \
         mixin M on A<int> implements B<int> {} \
         class X extends A<double> with M implements B<String> {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// The drop is clause-wide: a satisfied sibling mixin in the same `with`
/// list contributes nothing once a later application fails (probed
/// 3.13.7 — `B<int>` from M1 never reaches the merge).
#[test]
fn one_failure_drops_satisfied_siblings_too() {
    let report = report_ops(
        "class A<T> {} \
         class B<T> {} \
         class C extends A<int> {} \
         mixin M1 implements B<int> {} \
         mixin M2 on A<String> implements B<String> {} \
         class X extends C with M1, M2 implements B<double> {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// A satisfied application still merges the mixin's interfaces: `B<int>`
/// arrives only through `M`'s `implements` face and conflicts with the
/// declaration's own `implements B<String>` (probed 3.13.7).
#[test]
fn satisfied_application_still_merges_the_mixin_interfaces() {
    let report = report_ops(
        "class A<T> {} \
         class B<T> {} \
         mixin M on A<int> implements B<int> {} \
         class X extends A<int> with M implements B<String> {}\n",
    );
    assert_eq!(
        conflict_rows(&report),
        // The declaration's `implements` face collects before the `with`
        // chain (the official clause order), so `B<String>` stands first.
        ["The class 'X' can't implement both 'B<String>' and 'B<int>' because the type arguments are different."]
    );
}

/// An earlier mixin's interfaces satisfy a later constraint — the
/// official `enclosingClass.mixins[i]` probe (upstream
/// `test_class_matchingInterface_inPreviousMixin`): valid, silent.
#[test]
fn a_previous_mixin_satisfies_the_next_constraint() {
    let report = report_ops(
        "abstract class A<T> {} \
         class B {} \
         mixin M1 implements A<B> {} \
         mixin M2<T> on A<T> {} \
         class C extends Object with M1, M2 {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// `extends A` (the `A<dynamic>` reading) satisfies `on A<T>` with
/// T=dynamic — the inferred `M<dynamic>` is a valid application
/// (probed 3.13.7: no issues at all).
#[test]
fn a_raw_supertype_satisfies_the_inferred_constraint() {
    let report = report_ops(
        "abstract class A<T> {} \
         mixin M<T> on A<T> {} \
         class C extends A with M {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// The inference bound fallback substitutes through the partial
/// solution: `M6<int, int>`, not `M6<int, S>`
/// (`mixin_declaration_inference_valid_mixin_applications`'s A31 — the
/// oracle reports nothing here).
#[test]
fn the_bound_fallback_substitutes_through_the_partial_solution() {
    let report = report_ops(
        "class I<S> {} \
         class C0<T> extends I<T> {} \
         mixin M6<S, T extends S> on I<S> {} \
         mixin A31Mixin on C0<int>, M6<int, int> {} \
         class A31 = C0<int> with M6, A31Mixin;\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// The hierarchy loop reads the pre-seeded empty `_Hierarchy` on the
/// cyclic re-entry, so neither declaration reports a conflict (the
/// oracle answers `recursive_interface_inheritance` instead —
/// `conflicting_generic_interfaces_hierarchy_loop_infinite_test` and
/// `RecursiveInterfaceInheritanceTest/test_class_implements_generic_
/// typeArgument`).
#[test]
fn a_hierarchy_loop_collects_no_interfaces() {
    let report = report_ops(
        "class A<T> implements B<List<T>> {} \
         class B<T> implements A<List<T>> {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// The constraint subtype check reads the finished hierarchy — `Local`
/// satisfies `Dir` through the very mixin being applied (the official
/// `isSubtypeOf` consults `element.mixins`, already attached), so the
/// application stands and the instantiated `on` chain conflicts
/// (`regress32353`'s `_LocalDirectory` shape — oracle: the row stays).
#[test]
fn the_subtype_check_reads_the_finished_hierarchy() {
    let report = report_ops(
        "class F<T, D> {} \
         class Dir {} \
         class ioDir {} \
         mixin Fwd<T extends Dir> on F<T, ioDir> implements Dir {} \
         class Local extends F<Local, ioDir> with Fwd {}\n",
    );
    assert_eq!(
        conflict_rows(&report),
        ["The class 'Local' can't implement both 'F<Local, ioDir>' and 'F<Dir, ioDir>' because the type arguments are different."]
    );
}
