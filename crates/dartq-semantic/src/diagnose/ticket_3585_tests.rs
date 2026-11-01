//! The #3585 positive controls for the augment-fragment instance-scope
//! filter of `conflicting_static_and_instance`.
//!
//! The upstream contract (member_duplicate_definition_verifier.dart,
//! 3.13.7) has two separable halves, and the filter must touch only one:
//!
//! - **Claim side** (`_checkUnit` :536-557 — a complete pass over every
//!   declaration before any probe runs): `_checkDuplicateIdentifier`
//!   returns at :307-309 for a fragment whose `isAugmentation` is set, and
//!   claims every other fragment at :316-319. `isAugmentation` is the
//!   member's OWN `augment` keyword (`fragment.isAugmentation =
//!   node.augmentKeyword != null`, element_builder.dart) — a member
//!   written WITHOUT the keyword inside an `augment class` body is an
//!   introduction the fragment makes into the merged element, and it
//!   joins `instanceScope` like any introduction.
//! - **Probe side** (`_checkUnitStatic` :559-583, a second complete pass):
//!   `_checkClassStatic` :179-234 probes EVERY static member of every
//!   declaration against the merged `instanceScope`, with no
//!   `isAugmentation` filter at all.
//!
//! The two passes run back to back over the whole unit, so a claim made by
//! a later fragment is visible to an earlier declaration's probe: order
//! within the unit never changes the verdict.
//!
//! The pins here hold the contract's positive half fixed while the filter
//! for the negative half (the two `#[ignore]`d #3583 pins) lands: dropping
//! augmenting members from the instance scope must not drop the
//! non-augmenting members a fragment introduces, and must not touch the
//! probe side.

use super::test_rig::report_ops_experiments_with;
use super::FileReport;
use dartq_language::Feature;

fn conflicts(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "conflicting_static_and_instance")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn conflicts_augmented(source: &str) -> Vec<(u32, u32, String)> {
    let report = report_ops_experiments_with(source, &[], None, &[Feature::Augmentations]);
    conflicts(&report)
}

/// `class C { static int foo = 0; } augment class C { int foo = 0; }`: the
/// fragment's `int foo` carries no `augment` keyword, so it is an
/// introduction that claims `foo` in the merged instance scope, and the
/// introduction's static `foo` collides with it — exactly one row, at the
/// static member's name (offset 23 is `foo` in `  static int foo`).
#[test]
fn non_augment_field_in_augment_container_still_claims_and_reports() {
    let rows = conflicts_augmented(
        "class C {\n\
         \x20 static int foo = 0;\n\
         }\n\
         augment class C {\n\
         \x20 int foo = 0;\n\
         }\n",
    );
    assert_eq!(rows.len(), 1, "fragment introduction claims: {rows:?}");
    assert_eq!((rows[0].0, rows[0].1), (23, 3), "anchor is the static probe: {rows:?}");
    assert!(rows[0].2.contains("'C.foo'"), "{rows:?}");
}

/// The method spelling of the same contract: `void bar() {}` inside the
/// `augment class` body is an introduction, and the static `bar` of the
/// introduction reports once against it.
#[test]
fn non_augment_method_in_augment_container_still_claims_and_reports() {
    let rows = conflicts_augmented(
        "class C {\n\
         \x20 static void bar() {}\n\
         }\n\
         augment class C {\n\
         \x20 void bar() {}\n\
         }\n",
    );
    assert_eq!(rows.len(), 1, "fragment introduction claims: {rows:?}");
    assert!(rows[0].2.contains("'C.bar'"), "{rows:?}");
}

/// The probe side is augment-agnostic and the claim side counts fragment
/// introductions: `class C { int foo = 0; } augment class C { static int
/// foo = 0; }` — the fragment's static `foo` (no keyword of its own) is a
/// probe, the introduction's instance `foo` is in scope, one row at the
/// fragment's static member. The sibling #3583 pin
/// `augment_static_member_with_target_still_reports` covers the
/// `augment static` spelling of the same probe; both must keep reporting.
#[test]
fn non_augment_static_in_augment_container_still_probes_and_reports() {
    let rows = conflicts_augmented(
        "class C {\n\
         \x20 int foo = 0;\n\
         }\n\
         augment class C {\n\
         \x20 static int foo = 0;\n\
         }\n",
    );
    assert_eq!(rows.len(), 1, "fragment static probe: {rows:?}");
    assert!(rows[0].2.contains("'C.foo'"), "{rows:?}");
    // The anchor is the fragment's own static member name: the
    // introduction (`class C {\n  int foo = 0;\n}\n`, 27 bytes) plus the
    // fragment head (`augment class C {\n`, 18) plus `  static int ` (13).
    assert_eq!((rows[0].0, rows[0].1), (58, 3), "anchor is the fragment probe: {rows:?}");
}
