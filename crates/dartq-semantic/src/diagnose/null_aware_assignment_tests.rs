//! The null-aware assignment node's own static type: `d?.v = e` types as
//! the RHS face made nullable (`AssignmentExpression.staticType` — the
//! write is skipped when the receiver is null, so the assignment evaluates
//! to `null` and the node widens to `T2?`), while a plain `d.v = e` keeps
//! the bare RHS face. The `??=` twin (`resolveIfNull`'s `nullAware` join,
//! `fold_if_null_assignment`) widens the same way, from its own
//! `UP(NonNull(T1), T2)` face.
//!
//! Probed against `dart analyze` 3.13.7 (the pinned oracle at
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart`). The oracle anchors these
//! rows at the inner assignment expression (`d?.v = e`, the parenthesized
//! declaration's unwrapped extent); the pins below assert the value-type
//! face, which is the split this module owns — the parenthesized-group
//! anchor is the separate `checkForAssignableExpressionAtType` unwrap.
//!
//! The gate is syntactic, not nullability-based (all shapes probed): a
//! nullable receiver widens, a NON-nullable receiver still widens (`?.`
//! only warns there), a plain target keeps the bare face, and a static
//! qualifier target keeps the bare face because `C?.staticM` never
//! short-circuits (`assignment_expression_resolver.dart`'s
//! `analyzePropertyTargetReceiver` returns early for `StaticQualifierImpl`).
//! "Warned `?.`" is not the gate either: an extension override over a
//! non-null receiver warns the same way and still widens
//! (`EX(s)?.p ??= e` types `E?`). A cascade target (`c?..v ??= e`) types
//! the CASCADE's value, its receiver (`C?`), never the section's face.

use crate::diagnose::test_rig::report_ops;
use crate::diagnose::types::FileReport;

/// Every `invalid_assignment` row's message, in report order.
fn assignment_messages(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect()
}

/// The reporting direction: `F? f = (d?.v = e)` over a null receiver types
/// the assignment `E?`, so the declaration reports `E?` against `F?` —
/// never the un-nulled `E` the bare RHS face would give.
///
/// Probed: one row, `A value of type 'E?' can't be assigned to a variable
/// of type 'F?'.` (oracle col 11, length 8 — the inner `d?.v = e`).
#[test]
fn null_aware_assignment_value_types_the_rhs_nullable() {
    let fixture = "class D { E v; D(this.v); }\n\
         class E {}\n\
         class F {}\n\
         void test(D? d, E e) {\n\
         \x20 F? f = (d?.v = e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'E?' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The silent twin: a conforming value type stays silent through the fold —
/// `G? <: F?` for `G extends F`, so the nullable widening must not create a
/// report of its own.
///
/// Probed: no `invalid_assignment` row (only `unused_local_variable` on
/// `f`).
#[test]
fn null_aware_assignment_conforming_value_stays_silent() {
    let fixture = "class D { G v; D(this.v); }\n\
         class F {}\n\
         class G extends F {}\n\
         void test(D? d, G g) {\n\
         \x20 F? f = (d?.v = g);\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        assignment_messages(&report).is_empty(),
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The counter-direction that keeps the fold honest: a NON-null target keeps
/// the bare RHS face — `F? f = (d.v = e)` reports `E`, never `E?`. This is
/// the gate's own pin: without it the widening would spread to every plain
/// assignment and wrongly null the value type of conforming writes too.
///
/// Probed: one row, `A value of type 'E' can't be assigned to a variable of
/// type 'F?'.` (oracle col 11, length 7 — the inner `d.v = e`).
#[test]
fn plain_assignment_value_keeps_the_bare_rhs_face() {
    let fixture = "class D { E v; D(this.v); }\n\
         class E {}\n\
         class F {}\n\
         void test(D d, E e) {\n\
         \x20 F? f = (d.v = e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'E' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The static-qualifier counter-direction: `D?.staticE = e` never
/// short-circuits — the `?.` on a type spelling is an
/// `invalid_null_aware_operator` warning and the access resolves as a plain
/// `.`, so the node types the bare `E`. Widening here would be a wrong
/// spread driven by the shared `null_aware_members` latch, which records
/// the selector for every `?.` merge (the warning is emitted separately, at
/// the same merge, from the receiver's `Container` classification).
///
/// Probed: one `invalid_assignment` row, `A value of type 'E' can't be
/// assigned to a variable of type 'F?'.` (plus the
/// `invalid_null_aware_operator` warning, filtered out here).
#[test]
fn static_qualifier_target_keeps_the_bare_rhs_face() {
    let fixture = "class D {\n\
         \x20 static E staticE = E();\n\
         }\n\
         class E {}\n\
         class F {}\n\
         void test(E e) {\n\
         \x20 F? f = (D?.staticE = e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'E' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The `??=` static-qualifier counter-direction (`fold_if_null_assignment`'s
/// own copy of the gate): `D?.staticE ??= e` does not short-circuit, so the
/// node keeps the non-null `UP(NonNull(E), E)` = `E`.
///
/// Probed: one `invalid_assignment` row, `A value of type 'E' can't be
/// assigned to a variable of type 'F?'.` (plus the
/// `invalid_null_aware_operator` warning, filtered out here). Pre-fix the
/// same expression typed `E?`.
#[test]
fn if_null_static_target_keeps_the_non_null_face() {
    let fixture = "class D {\n\
         \x20 static E staticE = E();\n\
         }\n\
         class E {}\n\
         class F {}\n\
         void test(E e) {\n\
         \x20 F? f = (D?.staticE ??= e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'E' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The `??=` positive direction: a null-aware INSTANCE target still
/// short-circuits — `UP(NonNull(E), E)` is `E`, then the `nullAware` join
/// makes it `E?`.
///
/// Probed: one `invalid_assignment` row, `A value of type 'E?' can't be
/// assigned to a variable of type 'F?'.`
#[test]
fn if_null_null_aware_target_keeps_the_nullable_face() {
    let fixture = "class D { E v; D(this.v); }\n\
         class E {}\n\
         class F {}\n\
         void test(D? d, E e) {\n\
         \x20 F? f = (d?.v ??= e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'E?' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The `??=` anti-spread direction: a plain target carries no `?.` and
/// keeps the bare `UP` face — `E`, never `E?`.
///
/// Probed: one `invalid_assignment` row, `A value of type 'E' can't be
/// assigned to a variable of type 'F?'.`
#[test]
fn if_null_plain_target_keeps_the_non_null_face() {
    let fixture = "class D { E v; D(this.v); }\n\
         class E {}\n\
         class F {}\n\
         void test(D d, E e) {\n\
         \x20 F? f = (d.v ??= e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'E' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// A cascade target (`c?..v ??= e`) is a third case: the `??=` section's
/// own value is never the cascade's, so the parenthesized group types the
/// CASCADE expression — its receiver `C?`, widened by the `?..` — never the
/// section's `E`. Pinned in both directions so the `??=` latch cannot start
/// leaking into cascade groups.
///
/// Probed: one `invalid_assignment` row, `A value of type 'C?' can't be
/// assigned to a variable of type 'F?'.` (oracle line 32, len 1 — the
/// cascade's receiver).
#[test]
fn cascade_null_aware_if_null_keeps_the_cascade_receiver_face() {
    let fixture = "class C { E v; C(this.v); }\n\
         class E {}\n\
         class F {}\n\
         void test(C? c, E e) {\n\
         \x20 F? f = (c?..v ??= e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'C?' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The plain cascade twin: no `?..` short-circuit, and still the cascade
/// receiver's face rather than the section's.
///
/// Probed: one `invalid_assignment` row, `A value of type 'C?' can't be
/// assigned to a variable of type 'F?'.`
#[test]
fn cascade_plain_if_null_keeps_the_cascade_receiver_face() {
    let fixture = "class C { E v; C(this.v); }\n\
         class E {}\n\
         class F {}\n\
         void test(C? c, E e) {\n\
         \x20 F? f = (c..v ??= e);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        assignment_messages(&report),
        vec!["A value of type 'C?' can't be assigned to a variable of type 'F?'."],
        "{:?}",
        super::test_rig::rows(&report)
    );
}
