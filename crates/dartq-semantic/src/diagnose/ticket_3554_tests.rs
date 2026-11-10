//! The #3554 pins — the augmentation fragment boundary survives collection.
//!
//! # The defect
//!
//! The collector folded an `augment` container's members into the merged
//! element row as ordinary introductory members (`fold_fragment` chained
//! every `fragment.members` entry), and the member rows carried no fact
//! distinguishing an augmentation fragment (its own `augment` modifier)
//! from an introduction. Three faces read that merged map and mis-fired:
//!
//! * the merged duplicate replay (`library`'s #3378 pass) treated every
//!   fragment-unit member as a claiming introduction, so an `augment`
//!   member repeating its introduction reported `duplicate_definition`
//!   (upstream returns before the scope claim for augmentation fragments —
//!   member_duplicate_definition_verifier.dart:307-309);
//! * `populate` re-entered the folded augment member as a COMPLETE
//!   INTRODUCTION of the same property, so the augmentation chain's
//!   "introduction" was the augment's own folded copy —
//!   `declaration_already_complete` fired against itself while the true
//!   getter-less/setter-less faces went unreported;
//! * an augment member whose declaration did not exist folded in as an
//!   introduction, so `augmentation_without_declaration` never fired.
//!
//! Separately, `end_primary_constructor` registered no
//! `AugTarget { kind: Constructor }` row, so every augmentation of a
//! primary constructor resolved no chain at all and reported
//! `augmentation_without_declaration` against a declaration that exists.
//!
//! # The model
//!
//! A member declaration is an augmentation fragment iff it carries its own
//! `augment` modifier (`fragment.isAugmentation =
//! node.augmentKeyword != null`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/summary2/element_builder.dart). A member of an
//! `augment` container WITHOUT the modifier is an introduction that
//! fragment makes — it claims into the merged element's scope exactly like
//! the #3378 pins assert. [`MemberEntry::is_augment`] carries the bit;
//! `fold_fragment` drops marked members from the merged map; the #3378
//! replay skips them. A primary constructor is a complete origin
//! declaration (`isPrimary`/`isComplete`), except an `augment extension
//! type` head's representation primary, which is itself an augmentation.
//!
//! # The pins
//!
//! Each pin's expected rows were probed against the reference analyzer
//! (3.13.7, machine format) on the exact source shown:
//!
//! ① two fragments each declaring the same member with the `augment`
//! modifier — no `duplicate_definition`; the two
//! `declaration_already_complete` rows (the complete intro's induced
//! getter and setter) stand;
//! ② the same repeat WITHOUT the member `augment` modifier (the #3378
//! shape) still reports `duplicate_definition` — the introduction the
//! fragment makes claims as usual;
//! ③ an augment member whose declaration exists stays silent on the
//! duplicate/conflict faces (same unit — the folded copy is gone);
//! ④ an augment member with NO declaration reports
//! `augmentation_without_declaration`;
//! ⑤ a primary-constructor augmentation with a mismatched arity reports
//! the signature-mismatch code (the chain resolves the primary's target
//! row now), and a matching one stays silent;
//! ⑥ an extension type's representation primary augmented by `ET.new` and
//! vice versa stays silent (the unnamed and `.new` spellings share one
//! target row);
//! ⑦ a variable augmentation over a setter-only introduction: the bogus
//! `declaration_already_complete` pair dies (the augment's folded copy no
//! longer masquerades as the introduction).

use super::test_rig::{
    code_names, report_ops_augment, report_ops_augment_experiments, report_ops_experiments_with,
    rows,
};

/// ①a Cross-unit: the fragment's `augment int x` repeating the
/// introduction reports NO `duplicate_definition` — the analyzer emits the
/// two `declaration_already_complete` rows (complete intro, complete
/// augment) and nothing else on the duplicate face.
#[test]
fn augment_member_repeating_an_introduction_never_claims() {
    let report = report_ops_augment(
        "class C { int x = 1; }\n",
        "augment class C { augment int x = 2; }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
    // The oracle's own rows for this source: the augment's induced getter
    // and setter each report against the already-complete introduction.
    let complete = names
        .iter()
        .filter(|name| **name == "declaration_already_complete")
        .count();
    assert_eq!(complete, 2, "{:#?}", rows(&report));
}

/// ①b Same unit: the folded augment member joins no merged map — no
/// `duplicate_definition`, and the two `declaration_already_complete`
/// rows (the oracle's exact count for this source) stand.
#[test]
fn same_unit_augment_member_folds_nowhere() {
    let report = report_ops_experiments_with(
        "class C { int x = 0; }\naugment class C { augment int x = 1; }\n",
        &[],
        None,
        &[dartq_language::Feature::Augmentations],
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
    let complete = names
        .iter()
        .filter(|name| **name == "declaration_already_complete")
        .count();
    assert_eq!(complete, 2, "{:#?}", rows(&report));
}

/// ② The fragment member WITHOUT its own modifier is an introduction the
/// fragment makes: the repeat reports `duplicate_definition` at the
/// fragment's declaration (the #3378 contract, probed unchanged).
#[test]
fn plain_fragment_member_repeating_an_introduction_still_claims() {
    let report = report_ops_augment(
        "class C { int x = 1; }\n",
        "augment class C { int x = 2; }\n",
    );
    assert!(
        code_names(&report).contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}

/// ③ An augment member whose declaration exists reports nothing on the
/// fragment faces: the introduction is incomplete (bodiless — a
/// declaration left for an augmentation to complete), the augmentation is
/// incomplete too, so the already-complete walk finds no complete
/// preceding fragment, the augment member claims nowhere, and the chain
/// resolves the introduction.
#[test]
fn augment_member_with_an_existing_declaration_is_silent() {
    let report = report_ops_experiments_with(
        "class C { void m(int a); }\naugment class C { augment void m(int a); }\n",
        &[],
        None,
        &[dartq_language::Feature::Augmentations],
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"duplicate_definition")
            && !names.contains(&"declaration_already_complete")
            && !names.contains(&"augmentation_without_declaration"),
        "{:#?}",
        rows(&report)
    );
}

/// ④ An augment member with NO declaration folds into no merged map and
/// claims no chain: `augmentation_without_declaration` fires at the
/// member's name (the oracle's one row for this source).
#[test]
fn augment_member_without_a_declaration_reports() {
    let report = report_ops_experiments_with(
        "class C { }\naugment class C { augment int y = 1; }\n",
        &[],
        None,
        &[dartq_language::Feature::Augmentations],
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"duplicate_definition")
            && !names.contains(&"declaration_already_complete"),
        "{:#?}",
        rows(&report)
    );
    assert!(
        names.contains(&"augmentation_without_declaration"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑤a A primary-constructor augmentation resolves the primary's target
/// row: a mismatched required-positional arity reports the signature
/// code (the oracle's one row for this source), never
/// `augmentation_without_declaration`.
#[test]
fn primary_constructor_augmentation_finds_its_target() {
    let report = report_ops_experiments_with(
        "class C1(int x);\naugment class C1 { augment C1(); }\n",
        &[],
        None,
        &[
            dartq_language::Feature::Augmentations,
            dartq_language::Feature::PrimaryConstructors,
        ],
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"augmentation_without_declaration"),
        "{:#?}",
        rows(&report)
    );
    assert!(
        names.contains(&"augmentation_required_positional_formal_parameter_count"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑤b A matching primary-constructor augmentation stays silent (probed:
/// the analyzer emits nothing for this source).
#[test]
fn matching_primary_constructor_augmentation_is_silent() {
    let report = report_ops_experiments_with(
        "class C2(int x);\naugment class C2 { augment C2(int x); }\n",
        &[],
        None,
        &[
            dartq_language::Feature::Augmentations,
            dartq_language::Feature::PrimaryConstructors,
        ],
    );
    assert!(
        !code_names(&report).contains(&"augmentation_without_declaration"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑥ An extension type's representation primary is one target under the
/// container's own name: augmenting it as `ET.new` (and a `.new`-spelled
/// primary augmented bare) stays silent in both directions (the
/// extension_types_A02 shape, probed silent).
#[test]
fn extension_type_primary_augmentations_resolve() {
    let report = report_ops_experiments_with(
        "extension type ET1(int id) {}\naugment extension type ET1 {\n  augment ET1.new(int id);\n}\nextension type ET2.new(int id) {}\naugment extension type ET2 {\n  augment ET2(int id);\n}\n",
        &[],
        None,
        &[dartq_language::Feature::Augmentations],
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"augmentation_without_declaration")
            && !names.contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑦ A variable augmentation over a setter-only introduction: the
/// augmentation induces a getter AND a setter; the setter's introduction
/// exists (incomplete — the augmentation completes it), the getter's does
/// not. The augment's folded copy must not masquerade as a complete
/// introduction — the bogus `declaration_already_complete` pair dies.
#[test]
fn variable_augmentation_over_a_setter_only_introduction() {
    let report = report_ops_experiments_with(
        "class C {\n  void set s(String _);\n}\naugment class C { augment var s = \"x\"; }\n",
        &[],
        None,
        &[dartq_language::Feature::Augmentations],
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"declaration_already_complete"),
        "{:#?}",
        rows(&report)
    );
    // The oracle's row for this source: the induced getter has no
    // declaration to augment.
    assert!(
        names.contains(&"augmentation_without_declaration"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑧ An enum's primary constructor is an augmentation target too (the
/// implicitly-const representation head), keyed under the enum's own
/// name: a mismatched arity reports the signature code, not
/// `augmentation_without_declaration`.
#[test]
fn enum_primary_constructor_augmentation_finds_its_target() {
    let report = report_ops_experiments_with(
        "enum E1(int x) { e0(0); }\naugment enum E1 {\n  ;\n  augment const E1();\n}\n",
        &[],
        None,
        &[
            dartq_language::Feature::Augmentations,
            dartq_language::Feature::PrimaryConstructors,
        ],
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"augmentation_without_declaration"),
        "{:#?}",
        rows(&report)
    );
    assert!(
        names.contains(&"augmentation_required_positional_formal_parameter_count"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑨ The cross-unit augment face keeps its #3378-consistent behavior for
/// executables: an `augment` method repeating a static introduction
/// claims nowhere (no `duplicate_definition`), with the experiments rig
/// the two-unit pins use.
#[test]
fn cross_unit_augment_executable_never_claims() {
    let report = report_ops_augment_experiments(
        "class C {\n  static void m() {}\n}\n",
        "augment class C { augment static void m(); }\n",
        &[dartq_language::Feature::Augmentations],
    );
    assert!(
        !code_names(&report).contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}
