//! The #3331 pins — `no_combined_super_signature`, the #3301 residual C:
//! the code the combined-signature cohort was waiting on (a registry entry
//! plus an emit face) before its remaining oracle rows could move.
//!
//! Upstream face (dart 3.13.7): `instance_member_inferrer.dart`
//! `_inferExecutable` runs on every method declaration carrying an implicit
//! type slot, feeds the overridden candidate set to
//! `combineSignatureTypes`, and records
//! `TopLevelInferenceErrorNoCombinedSuperSignature(candidateSignatures)`
//! when the merge answers null — `inheritance_override.dart`
//! `_reportNoCombinedSuperSignature` then prints the row at the method name
//! (`Can't infer missing types in '<class>' from overridden methods:
//! <candidates>.`, each candidate `Class.member (signature)`) and the
//! declared-face override check for that member is skipped (the oracle's
//! `continue`). Here the verdict rides the same candidate machinery the
//! inference pass uses: per-face hits under the written name, the edge
//! substitution, and the `_getValidOverrides` survivor fold.
//!
//! # What this file does not pin (recorded, not closed)
//!
//! - the `<unknown>` candidateSignatures face: upstream prints it when the
//!   null comes from `_topMergeSignatureTypes` rather than a
//!   `CandidatesConflict`; the merge here keeps the first valid candidate on
//!   an unmodelled pair (the pre-port posture), so the verdict never lands
//!   there and the `<unknown>` text stays unreachable;
//! - getters, setters, and fields: the oracle checks
//!   `fragment is MethodFragmentImpl`, so only methods carry the row — the
//!   emit gate mirrors that (the `MemberKind::Method` check on the declared
//!   entry);
//! - a kind-mismatched candidate set suppresses upstream
//!   (`_allSameElementKind`'s gate) and here (the method-only hit filter).

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops};

/// Every `no_combined_super_signature` row of `report` as
/// `message @anchored-text`, in emission order; `fixture` is the source the
/// rig analyzed ahead of the shared core prelude.
fn combined_rows(fixture: &str, report: &FileReport) -> Vec<String> {
    let full = format!("{CORE_PRELUDE}{fixture}");
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "no_combined_super_signature")
        .map(|d| {
            format!(
                "{} @{}",
                d.message,
                &full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
            )
        })
        .collect()
}

/// The oracle's `test_conflictingParameter`: two uncombinable parameter
/// columns over an untyped method — the row lands at the method name with
/// both candidates spelled, and the member's `invalid_override` faces stay
/// silent (the oracle's `continue`).
#[test]
fn conflicting_parameter_reports_at_the_method_name() {
    let fixture = "abstract class A {\n\
         \x20 void foo(int x);\n\
         }\n\
         abstract class B {\n\
         \x20 void foo(double x);\n\
         }\n\
         abstract class C implements A, B {\n\
         \x20 foo(num x);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        combined_rows(fixture, &report),
        vec![
            "Can't infer missing types in 'C' from overridden methods: \
             A.foo (void Function(int)), B.foo (void Function(double)). @foo"
        ],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
}

/// The oracle's `test_conflictingReturnType`: the written return type
/// (`Never`) does not save the member — one implicit parameter slot is
/// enough, and the candidate list reads the declared signatures.
#[test]
fn conflicting_return_type_reports_with_the_written_return_kept() {
    let fixture = "abstract class A {\n\
         \x20 int foo(int x);\n\
         }\n\
         abstract class B {\n\
         \x20 double foo(int x);\n\
         }\n\
         abstract class C implements A, B {\n\
         \x20 Never foo(x);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        combined_rows(fixture, &report),
        vec![
            "Can't infer missing types in 'C' from overridden methods: \
             A.foo (int Function(int)), B.foo (double Function(int)). @foo"
        ],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
}

/// The oracle's `test_noInvalidOverrideErrors`: the conflict alone owns the
/// member — no `invalid_override` row accompanies it, the inference's
/// `dynamic` fallback being a face the override check never judges.
#[test]
fn conflicting_candidates_own_the_member_without_override_rows() {
    let fixture = "abstract class A {\n\
         \x20 String foo(String a);\n\
         }\n\
         abstract class B {\n\
         \x20 int foo(int a);\n\
         }\n\
         abstract class C implements A, B {\n\
         \x20 foo(a);\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        combined_rows(fixture, &report),
        vec![
            "Can't infer missing types in 'C' from overridden methods: \
             A.foo (String Function(String)), B.foo (int Function(int)). @foo"
        ],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
}

/// The same conflict over concrete classes: inference runs on every method
/// declaration, abstract or not — the corpus's common face.
#[test]
fn concrete_class_conflicting_candidates_report_too() {
    let fixture = "class A {\n\
         \x20 void foo(int x) {}\n\
         }\n\
         class B {\n\
         \x20 void foo(double x) {}\n\
         }\n\
         class C implements A, B {\n\
         \x20 foo(x) {}\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        combined_rows(fixture, &report),
        vec![
            "Can't infer missing types in 'C' from overridden methods: \
             A.foo (void Function(int)), B.foo (void Function(double)). @foo"
        ],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
}

/// A combinable candidate set stays silent: `B.foo` validly overrides
/// `A.foo` (int <: num on the return, num accepting int on the parameter),
/// so the combined signature exists and the member infers from it.
#[test]
fn combinable_candidate_set_stays_silent() {
    let report = report_ops(
        "abstract class A {\n\
         \x20 num foo(int x);\n\
         }\n\
         abstract class B {\n\
         \x20 int foo(num x);\n\
         }\n\
         abstract class C implements A, B {\n\
         \x20 foo(x);\n\
         }\n",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The single-candidate gate: one overridden member combines trivially
/// (`combineSignatureTypes`' `candidates.length == 1` arm) — nothing
/// reports, and the member infers its slots from that candidate.
#[test]
fn single_candidate_inference_stays_silent() {
    let report = report_ops(
        "abstract class A {\n\
         \x20 void foo(int x);\n\
         }\n\
         abstract class C implements A {\n\
         \x20 foo(x);\n\
         }\n",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The implicit-slot gate: a fully typed method never carries the row, even
/// over a candidate set that would conflict — the written signature resolves
/// the face cleanly against both members.
#[test]
fn fully_typed_method_over_conflicting_candidates_stays_silent() {
    let report = report_ops(
        "abstract class A {\n\
         \x20 void foo(int x);\n\
         }\n\
         abstract class B {\n\
         \x20 void foo(double x);\n\
         }\n\
         abstract class C implements A, B {\n\
         \x20 void foo(num x);\n\
         }\n",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}
