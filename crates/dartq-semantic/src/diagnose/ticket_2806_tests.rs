//! The #2806 `duplicate_definition` false-positive pins: the two
//! declaration-identity shapes the oracle keeps silent — wildcard type
//! parameters (`_` never collides, `checkTypeParameters`'s `isWildcard`
//! arm, duplicate_definition_verifier.dart:398-402) and `augment typedef`
//! (an augmentation fragment never re-declares its name, `checkUnit`'s
//! `!declaredFragment.isAugmentation` gate, :296-304) — plus the
//! true-positive guards around each skip (a real repeat in the same list,
//! an introductory typedef re-declaring an occupied name).

use crate::diagnose::test_rig::{report_ops, rows};
use crate::diagnose::types::FileReport;

/// The `duplicate_definition` rows of `report`, `@offset:len message`.
fn dup_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "duplicate_definition")
        .map(|d| {
            format!(
                "{} @{}:{} {}",
                d.code.name, d.range.offset, d.range.length, d.message
            )
        })
        .collect()
}

#[track_caller]
fn assert_no_dups(report: &FileReport) {
    let dups = dup_rows(report);
    assert!(
        dups.is_empty(),
        "expected duplicate_definition silence, got {dups:#?}\nall rows: {:#?}",
        rows(report)
    );
}

#[track_caller]
fn assert_single_dup(report: &FileReport, name: &str) {
    let dups = dup_rows(report);
    assert_eq!(
        dups.len(),
        1,
        "expected one duplicate_definition for {name}, got {dups:#?}\nall rows: {:#?}",
        rows(report)
    );
    assert!(
        dups[0].ends_with(&format!("The name '{name}' is already defined.")),
        "{dups:#?}"
    );
}

// —— shape A: wildcard type parameters never collide ————————————————

/// Every container header kind with repeated `_` parameters — including a
/// wildcard carrying a written bound — stays silent (the co19
/// `binding_A02_t08` shape).
#[test]
fn wildcard_type_parameters_never_collide_per_list() {
    let report = report_ops(
        "class C<_, _, _ extends num> {}\n\
         \x20 mixin M<_, _, _ extends num> {}\n\
         \x20 enum E<_, _, _ extends num> { e1 }\n\
         \x20 extension type ET<_, _, _ extends num>(int _) {}\n",
    );
    assert_no_dups(&report);
}

/// All three typedef spellings — the old-style function alias, the modern
/// alias, and a generalized function type's own `<_, _>` — stay silent
/// (`test_typeParameters_functionTypeAlias_wildcard` /
/// `test_typeParameters_genericTypedef_*_wildcard` /
/// `test_typeParameters_genericFunction_wildcard`).
#[test]
fn wildcard_type_parameters_in_typedef_shapes_stay_silent() {
    let report = report_ops(
        "typedef void F<_, _>();\n\
         \x20 typedef G<_, _> = int;\n\
         \x20 typedef H = void Function<_, _>();\n",
    );
    assert_no_dups(&report);
}

/// Top-level, member, and local-function `<_, _>` groups all stay silent
/// (`test_typeParameters_topLevelFunction_wildcard` /
/// `test_typeParameters_method_wildcard` /
/// `local_declaration_type_parameter_test`).
#[test]
fn wildcard_type_parameters_in_executables_stay_silent() {
    let report = report_ops(
        "void top<_, _>() {}\n\
         \x20 class C { void m<_, _>() {} }\n\
         \x20 void main() { void local<_, _>() {} }\n",
    );
    assert_no_dups(&report);
}

/// The corpus exemplar (`local_declaration_function_parameter_test`
/// 70:22): a formal whose function-type annotation repeats `<_, _>` —
/// the annotation's own type-variable list stays silent beside the
/// repeated `_` formals.
#[test]
fn wildcard_gft_type_parameters_stay_silent() {
    let report = report_ops("void g(void Function<_, _>(int _, int _) _) {}\n");
    assert_no_dups(&report);
}

/// The skip is wildcard-only: a real repeat in the same list still
/// reports (the mixed `<_, T, T>` list — the wildcard joins the capture,
/// the `T` collision reports).
#[test]
fn mixed_wildcard_list_still_reports_the_real_duplicate() {
    let report = report_ops("class A<_, T, T> {}\n");
    assert_single_dup(&report, "T");
}

/// The non-wildcard per-list repeat keeps its row (the
/// `type_parameter_duplicate_degrades_to_the_plain_row` face, re-pinned
/// as this ticket's true-positive guard).
#[test]
fn plain_type_parameter_duplicate_still_reports() {
    let report = report_ops("class C<T, T> {}\n");
    assert_single_dup(&report, "T");
}

// —— shape B: an `augment typedef` never re-declares its name ————————

/// The augmenting alias over an occupied class name stays silent on the
/// name face (`applying_augmentations_A01_t03` /
/// `test_typedef_augments_class`): the parser's `typedef_augmentation`
/// row is the only report.
#[test]
fn augment_typedef_after_class_name_stays_silent() {
    let report = report_ops("class A {}\naugment typedef A = int;\n");
    assert_no_dups(&report);
}

/// The augmenting alias over the introductory alias stays silent
/// (`augmenting_class_like_declarations_A08_t01`).
#[test]
fn augment_typedef_after_intro_typedef_stays_silent() {
    let report = report_ops("class C0 {}\ntypedef C1 = C0;\naugment typedef C1 = C0;\n");
    assert_no_dups(&report);
}

/// The container-head augment beside its introduction never reports the
/// name either (the family guard around the augmentation merge — the
/// intro's element keeps its fragment chain).
#[test]
fn augment_class_head_after_intro_stays_silent() {
    let report = report_ops("class C {}\naugment class C {}\n");
    assert_no_dups(&report);
}

/// An INTRODUCTORY typedef re-declaring an occupied class name keeps its
/// row — the skip is the augmentation fragment's, not the alias kind's.
#[test]
fn intro_typedef_redeclaring_a_class_still_reports() {
    let report = report_ops("class A {}\ntypedef A = int;\n");
    assert_single_dup(&report, "A");
}

/// Two introductory aliases of one name keep the second's row.
#[test]
fn repeated_intro_typedefs_still_report() {
    let report = report_ops("typedef A = int;\ntypedef A = String;\n");
    assert_single_dup(&report, "A");
}
