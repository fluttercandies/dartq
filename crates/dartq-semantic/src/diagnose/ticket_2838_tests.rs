//! The #2838 pins: two seams, one no-regression net.
//!
//! The alias-capture fix — `typedef F<X> = void Function<Y extends X>()`
//! keeps the declaring `<X>` as the alias's parameters and latches the
//! RHS's own `<Y extends X>` as the function type's signature variables
//! (the analyzer element model), so the use-site instantiation
//! substitutes the written arguments into the bounds and the
//! `relateTypeParameters` equality/count rules (#2834) decide over real
//! populated params.
//!
//! The raw-type compare bridge — a raw `A` at a same-head assignability
//! pair reads as its instantiate-to-bounds defaults
//! (`instantiateToBounds`, type_system.dart:719-731) instead of the
//! arity-mismatch silence, decided by the same covariant element rule.
//! The differential gate keys on `(severity, code, file)`, so these pins
//! assert row counts and codes, not message spellings (the raw side's
//! row renders its written spelling; the oracle spells the resolved
//! instantiation).

use crate::diagnose::test_rig::{report_ops, rows};
use crate::diagnose::types::FileReport;

/// The `invalid_assignment` rows of `report`, messages only.
fn assignment_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `argument_type_not_assignable` rows of `report`, messages only.
fn arg_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string())
        .collect()
}

/// All diagnostics of `report`, code names only — the silence pins.
fn codes(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_string())
        .collect()
}

/// The instantiation substitutes the RHS's own variables: `F<int>`
/// expands to `void Function<Y extends int>()`, so the pair against the
/// equal written signature is clean in both directions.
#[test]
fn alias_instantiation_substitutes_the_rhs_variables() {
    let report = report_ops(
        "typedef F<X> = void Function<Y extends X>();\n\
         \x20 void f(F<int> a, void Function<Y extends int>() b) {\n\
         \x20   a = b;\n\
         \x20   b = a;\n\
         \x20 }",
    );
    assert!(
        assignment_messages(&report).is_empty(),
        "rows: {:#?}",
        rows(&report)
    );
}

/// The instantiated bounds' decision seat: `F<int>` against a
/// `num`-bounded written signature reports — the instantiation `void
/// Function<Y extends int>()` is no subtype of `void Function<Y extends
/// num>()` (relateTypeParameters' equality fails) in NEITHER direction.
/// The element-model seam (#2849) landed it: the formal's `F<int>`
/// annotation joins the written-signature stash with the instantiated
/// signature (`instantiated_alias_sig` — the RHS's own variables with
/// the arguments substituted into their bounds, the analyzer's resolved
/// annotation type), so the equality rule decides over real populated
/// params and the message spells the resolved instantiations.
#[test]
fn alias_instantiated_bounds_decide_the_equality_rule() {
    let report = report_ops(
        "typedef F<X> = void Function<Y extends X>();\n\
         \x20 void f(F<int> a, void Function<Y extends num>() b) {\n\
         \x20   a = b;\n\
         \x20   b = a;\n\
         \x20 }",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'void Function<Y extends num>()' can't be assigned to a variable of type 'void Function<Y extends int>()'.".to_string(),
            "A value of type 'void Function<Y extends int>()' can't be assigned to a variable of type 'void Function<Y extends num>()'.".to_string(),
        ],
        "rows: {:#?}",
        rows(&report)
    );
}

/// A bound that mentions a CO-variable substitutes through the map:
/// `G<num>` over `typedef G<X extends num> = void Function<Z extends
/// X>()` reads `void Function<Z extends num>()`. The oracle reports the
/// `int`-bounded pair only — the same instantiated-stash seat as the
/// equality pin above (#2849), the substituted bound displayed.
#[test]
fn alias_bound_substitution_reaches_co_variable_bounds() {
    let report = report_ops(
        "typedef G<X extends num> = void Function<Z extends X>();\n\
         \x20 void f(G<num> a, void Function<Z extends num>() b,\n\
         \x20     void Function<Z extends int>() c) {\n\
         \x20   a = b;\n\
         \x20   a = c;\n\
         \x20 }",
    );
    assert_eq!(
        assignment_messages(&report),
        vec![
            "A value of type 'void Function<Z extends int>()' can't be assigned to a variable of type 'void Function<Z extends num>()'.".to_string(),
        ],
        "rows: {:#?}",
        rows(&report)
    );
}

/// The raw-type bridge reports the mismatch direction: a raw `A`
/// (defaults `A<num>` through the `T extends num` bound) does not fit
/// `A<int>`.
#[test]
fn raw_source_reads_as_its_bound_default() {
    let report = report_ops(
        "class A<T extends num> {}\n\
         \x20 void take(A<int> w) {}\n\
         \x20 void main() {\n\
         \x20   A a = A();\n\
         \x20   take(a);\n\
         \x20 }",
    );
    assert_eq!(arg_messages(&report).len(), 1, "rows: {:#?}", rows(&report));
}

/// The clean direction stays clean: the raw head against its own bound
/// default (`A<num>` parameter) assigns.
#[test]
fn raw_source_matching_the_default_stays_clean() {
    let report = report_ops(
        "class A<T extends num> {}\n\
         \x20 void give(A<num> w) {}\n\
         \x20 void main() {\n\
         \x20   A a = A();\n\
         \x20   give(a);\n\
         \x20 }",
    );
    assert!(
        arg_messages(&report).is_empty(),
        "rows: {:#?}",
        rows(&report)
    );
}

/// The target-side fill: a written source into a raw target reads the
/// target's defaults — `B<num>` into raw `B` (defaults `B<int>`)
/// reports, the mirrored `A<int>` into raw `A` (defaults `A<num>`)
/// stays clean.
#[test]
fn raw_target_side_fills_from_written() {
    let report = report_ops(
        "class A<T extends num> {}\n\
         \x20 class B<T extends int> {}\n\
         \x20 void main() {\n\
         \x20   A<int> ai = A<int>();\n\
         \x20   A ra = ai;\n\
         \x20   B<num> bn = B<num>();\n\
         \x20   B rb = bn;\n\
         \x20 }",
    );
    let messages = assignment_messages(&report);
    assert_eq!(messages.len(), 1, "rows: {:#?}", rows(&report));
    assert!(
        messages[0].contains("B<num>"),
        "the reported pair is the B one: {messages:?}"
    );
}

/// An unbounded parameter defaults to `dynamic`: the raw head into a
/// `C<int>` parameter reports, and `C<int>` into a raw target stays
/// clean (`int <: dynamic`).
#[test]
fn unbounded_default_is_dynamic_and_decides() {
    let report = report_ops(
        "class C<T> {}\n\
         \x20 void take(C<int> w) {}\n\
         \x20 void main() {\n\
         \x20   C c = C();\n\
         \x20   take(c);\n\
         \x20   C<int> ci = C<int>();\n\
         \x20   C r = ci;\n\
         \x20 }",
    );
    assert_eq!(arg_messages(&report).len(), 1, "rows: {:#?}", rows(&report));
    assert!(
        assignment_messages(&report).is_empty(),
        "rows: {:#?}",
        rows(&report)
    );
}

/// A poisoned parameter bound (a record bound the model cannot spell)
/// keeps the arity silence — the defaulted dynamic the snapshot read is
/// a default the written shape never spells, the false-positive
/// direction.
#[test]
fn poisoned_bound_keeps_the_silence() {
    let report = report_ops(
        "class P<T extends (int, int)> {}\n\
         \x20 void main() {\n\
         \x20   P<int> pi = P<int>();\n\
         \x20   P p = pi;\n\
         \x20 }",
    );
    assert!(
        codes(&report)
            .iter()
            .all(|code| code != "invalid_assignment"),
        "rows: {:#?}",
        rows(&report)
    );
}

/// A written-vs-written arity mismatch is the `wrong_number` family's
/// residue — the bridge never fills it.
#[test]
fn written_arity_mismatch_stays_the_wrong_number_family() {
    let report = report_ops(
        "class A<T extends num, U> {}\n\
         \x20 void main() {\n\
         \x20   A<int> one = A<int>();\n\
         \x20   A<int, String> two = A<int, String>();\n\
         \x20   two = one;\n\
         \x20 }",
    );
    assert!(
        assignment_messages(&report).is_empty(),
        "rows: {:#?}",
        rows(&report)
    );
    assert!(
        codes(&report)
            .iter()
            .any(|code| code.starts_with("wrong_number_of_type_arguments")),
        "rows: {:#?}",
        rows(&report)
    );
}
