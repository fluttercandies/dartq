//! The #3404 pins — the two override faces the recon cluster named:
//!
//! - **The `no_combined_super_signature` gate** (`overrides.rs`
//!   `check_override`'s head, `no_combined_super_candidates`): a METHOD with
//!   an implicit type slot whose multi-candidate super-face set has no
//!   combined signature reports at the member name and the declared-face
//!   loop below skips the member entirely (upstream Dart SDK source,
//!   `pkg/analyzer/lib/src/error/inheritance_override.dart`
//!   `_reportNoCombinedSuperSignature` + `_checkDeclaredMember`'s
//!   `continue`, fed by `pkg/analyzer/lib/src/summary2/instance_member_inferrer.dart`
//!   `_inferExecutable`'s null `combineSignatureTypes`). The inference's
//!   `dynamic` fallback is never a face worth judging, so the skip is the
//!   contract: without it the loop fabricates `invalid_override` rows the
//!   oracle does not report.
//! - **The declared-`covariant` induced setter** (`overrides.rs`'s field
//!   arm): a field's synthesized setter face carries the declaring site's
//!   own `covariant` keyword (`element_builder.dart`'s
//!   `valueFragment.isExplicitlyCovariant`) in addition to the propagated
//!   marks, so a `covariant`-declared field narrowing a super setter's
//!   parameter is legal (upstream `correct_override.dart`'s covariant
//!   exemption), while the same field without the keyword still reports.
//!
//! Both faces landed with #3345 (the per-face interface map); these pins
//! lock the contracts so a later edit cannot silently restore either
//! direction of the regression.
//!
//! Upstream references are paths inside the pinned Dart SDK source tree;
//! the fixtures restate the corpus shapes verbatim.

use super::FileReport;
use super::test_rig::report_ops;

/// `"code: message"` per diagnostic, in emission order — the message is
/// part of the pin because the `no_combined_super_signature` rows render
/// their candidate list, not just their code.
fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// The gate fires on a two-`implements`-face conflict and the declared-face
/// loop skips the member: exactly ONE row, and it is the combined-signature
/// error — never the `invalid_override` the `dynamic` fallback would break
/// against every face. Proportional-parameter `int`/`double` are unrelated,
/// so no candidate survives (`_getValidOverrides` answers the empty set).
#[test]
fn no_combined_candidate_skips_the_declared_face_loop() {
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
        rows(&report),
        vec![
            "no_combined_super_signature: Can't infer missing types in 'C' from overridden \
             methods: A.foo (void Function(int)), B.foo (void Function(double))."
        ],
        "{:?}",
        report.diagnostics
    );
}

/// The same gate with the conflict on the RETURN face and an implicit
/// parameter slot: `Never foo(x)` over `int foo(int x)` / `double foo(int x)`
/// still reports the combined-signature error and only that row — the
/// resolved return type does not make the signature combinable
/// (upstream `test_conflictingReturnType`).
#[test]
fn no_combined_candidate_reports_on_a_return_type_conflict() {
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
        rows(&report),
        vec![
            "no_combined_super_signature: Can't infer missing types in 'C' from overridden \
             methods: A.foo (int Function(int)), B.foo (double Function(int))."
        ],
        "{:?}",
        report.diagnostics
    );
}

/// The mixin shape: the candidate set comes from the `on` constraints (a
/// mixin's only super faces in this model), and the gate fires there too —
/// the `inconsistent_inheritance` row is a separate face owned elsewhere,
/// so this pin asserts the combined-signature row plus the absence of any
/// declared-face-loop emission.
#[test]
fn no_combined_candidate_fires_on_a_mixin_on_clause() {
    let fixture = "class A {\n\
         \x20 void bar(int p) {}\n\
         }\n\
         class B {\n\
         \x20 void bar(double p) {}\n\
         }\n\
         mixin M on A, B {\n\
         \x20 void bar(p) {}\n\
         }\n";
    let report = report_ops(fixture);
    let combined: Vec<String> = rows(&report)
        .into_iter()
        .filter(|row| row.starts_with("no_combined_super_signature:"))
        .collect();
    assert_eq!(
        combined,
        vec![
            "no_combined_super_signature: Can't infer missing types in 'M' from overridden \
             methods: A.bar (void Function(int)), B.bar (void Function(double))."
        ],
        "{:?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "invalid_override"),
        "the declared-face loop must skip a no-combined member: {:?}",
        report.diagnostics
    );
}

/// A single candidate combines trivially (`candidates.len() <= 1`), so the
/// gate stays silent and the member infers from that one face — the
/// over-fire direction of the same decision point.
#[test]
fn single_candidate_super_face_stays_silent() {
    let fixture = "abstract class A {\n\
         \x20 void foo(int x);\n\
         }\n\
         abstract class C implements A {\n\
         \x20 foo(x);\n\
         }\n";
    let report = report_ops(fixture);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// A `covariant`-declared field marks its induced setter's parameter
/// directly, so narrowing the inherited `num?` to `int?` is legal — the
/// upstream covariant exemption skips the contravariance arm instead of
/// reporting `invalid_override`. Silent is the whole assertion, and the
/// `int`/`num` pair is decidable (the prelude declares `int extends num`),
/// so the silence is the exemption and not an undecidable suppression — the
/// twin pin below reports on the same shape without the keyword.
#[test]
fn declared_covariant_field_induced_setter_is_exempt() {
    let fixture = "class B {\n\
         \x20 void set s4(num? x) {}\n\
         }\n\
         class C extends B {\n\
         \x20 covariant int? s4;\n\
         }\n";
    let report = report_ops(fixture);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The same widening without the keyword still reports on the setter face:
/// the exemption is the declaring site's `covariant`, not a blanket pass for
/// field-derived setters.
#[test]
fn non_covariant_field_induced_setter_still_reports() {
    let fixture = "class B {\n\
         \x20 void set s4(num? x) {}\n\
         }\n\
         class C extends B {\n\
         \x20 int? s4;\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report),
        vec![
            "invalid_override: The setter 'C.s4' ('void Function(int?)') isn't a valid override \
             of 'B.s4' ('void Function(num?)')."
        ],
        "{:?}",
        report.diagnostics
    );
}

/// The propagated arm of the same flag: a `covariant` on the OVERRIDDEN
/// field marks the inherited setter face, and the declared written setter's
/// narrowing is exempt (`_inheritCovariance`; upstream
/// `test_setter_overrides_abstract_field_covariant_valid`).
#[test]
fn inherited_covariant_field_marks_the_overriding_setter() {
    let fixture = "abstract class A {\n\
         \x20 abstract covariant num x;\n\
         }\n\
         abstract class B implements A {\n\
         \x20 int get x;\n\
         \x20 void set x(int value);\n\
         }\n";
    let report = report_ops(fixture);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}
