//! The #3516 pins — a cross-unit `augment` fragment's declarations join the
//! merged element face, not just its members.
//!
//! # The defect
//!
//! `build_fragments` merged one column of an element's face: `members`
//! (#3418). An `augment` written in a *part* file (a second unit of the
//! library) therefore contributed its members but not its enum constants,
//! constructors, `const`-constructor presence, `extends` edge or
//! `with`/`implements`/`on` edges — so `E.e1` declared only in a part was
//! `undefined_enum_constant`, the switch witness set missed it, a part
//! constructor neither killed the implicit default nor resolved, a part
//! `with M` inherited nothing, and a part `const` constructor did not make
//! `const C()` legal.
//!
//! `library.rs` now folds every column (`admit_fragment`) and re-derives the
//! implicit default from the element's summed constructor declarations
//! (`CtorDeclarations`), the cross-unit half of the same-unit fold
//! `collector::Builder::fold_fragment` performs.
//!
//! # The rig
//!
//! Two units of ONE library ([`report_ops_augment_intro_experiments`], the
//! part-file shape: the introduction in `main.dart`, the `augment` heads in
//! a second unit), reporting the INTRODUCING unit — that is where the merged
//! face is read. Every expectation was checked against the oracle
//! (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze` over a package with
//! `analysis_options.yaml` `analyzer: enable-experiment: [augmentations]`
//! and the fragments in a real `part`/`part of` pair); each pin names the
//! verdict it encodes.

use super::test_rig::{code_names, report_ops_augment_intro_experiments, rows};

/// One two-unit fixture with the `augmentations` experiment enabled,
/// reporting the introducing unit.
fn augment_part(main: &str, part: &str) -> super::FileReport {
    report_ops_augment_intro_experiments(main, part, &[dartq_language::Feature::Augmentations])
}

/// No `undefined_*` row at all — the merged face resolved every read.
#[track_caller]
fn assert_no_undefined(report: &super::FileReport) {
    assert!(
        !code_names(report)
            .iter()
            .any(|name| name.starts_with("undefined_")),
        "{:#?}",
        rows(report)
    );
}

/// ① An enum constant declared only in the part is the element's constant:
/// `E.e1` written in the introduction resolves.
///
/// Oracle: `enum E { e0; }` in the part-file library plus `augment enum E {
/// e1; }` in the part, read as `E.e1` — no issues (`enums_A01_t01`).
#[test]
fn part_enum_constant_is_readable_from_the_introduction() {
    let report = augment_part(
        "enum E { e0; }\n\
         void reads() {\n  Object? a = E.e0;\n  Object? b = E.e1;\n  a;\n  b;\n}\n",
        "augment enum E { e1; }\n",
    );
    assert_no_undefined(&report);
}

/// ② The part's constant joins the enum's WITNESS set: a switch that covers
/// only the introduction's constant is non-exhaustive, and the report names
/// the part's constant.
///
/// Oracle: that switch reports `non_exhaustive_switch_statement` "since it
/// doesn't match the pattern 'E.e1'".
///
/// **Ignored — consumer gap, not a merge gap.** The merged face carries the
/// constant (`part_enum_constant_is_readable_from_the_introduction` is
/// green), but `switch_tracker::locate` answers an enum declared in the
/// DIAGNOSING file from that file's own row first and only then falls back
/// to the merged face, so a switch written in the introducing unit still
/// enumerates `[e0]`. Measured: the report is empty (no
/// `non_exhaustive_switch_statement` row) for this fixture. Owner:
/// `switch_tracker.rs` — `locate` must ask the element face, not
/// `index.get(file)`.
#[test]
#[ignore = "consumer gap: switch_tracker::locate prefers the unmerged local row — owner switch_tracker.rs"]
fn part_enum_constant_joins_the_witness_set() {
    let report = augment_part(
        "enum E { e0; }\n\
         void test(E e) {\n  switch (e) {\n    case E.e0:\n  }\n}\n",
        "augment enum E { e1; }\n",
    );
    assert!(
        code_names(&report).contains(&"non_exhaustive_switch_statement"),
        "{:#?}",
        rows(&report)
    );
}

/// ③ A constructor declared only in the part is the element's constructor:
/// the introduction's `C.named()` call resolves.
///
/// Oracle: `class C {}` in the library, `augment class C { C.named(); }` in
/// the part, `C.named();` — no issues. Fails while `constructors` carries
/// only the introduction's selectors.
#[test]
fn part_constructor_selector_is_callable() {
    let report = augment_part(
        "class C {}\n\
         void make() {\n  C.named();\n}\n",
        "augment class C {\n  C.named();\n}\n",
    );
    assert_no_undefined(&report);
}

/// ④ A named constructor declared only in the part suppresses the
/// introduction's IMPLICIT default: `C()` is undefined.
///
/// Oracle: that shape reports `new_with_undefined_constructor_default`
/// ("The class 'C' doesn't have an unnamed constructor"). The flag comes
/// from the element's summed declarations ([`super::CtorDeclarations`]),
/// never from either fragment's own `has_unnamed_constructor`.
#[test]
fn part_named_constructor_kills_the_implicit_default() {
    let report = augment_part(
        "class C {}\n\
         void make() {\n  C();\n}\n",
        "augment class C {\n  C.named();\n}\n",
    );
    assert!(
        code_names(&report).contains(&"new_with_undefined_constructor_default"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑤ A `const` constructor declared only in the part joins the const face:
/// `const C()` is legal.
///
/// Oracle: `class C {}` in the library plus `augment class C { const C(); }`
/// in the part, with `const k = C();` and a `const C();` statement — no
/// issues.
#[test]
fn part_const_constructor_joins_the_const_face() {
    let report = augment_part(
        "class C {}\n\
         const k = C();\n\
         void use() {\n  const C();\n  k;\n}\n",
        "augment class C {\n  const C();\n}\n",
    );
    assert!(
        !code_names(&report)
            .iter()
            .any(|name| name.starts_with("const_")),
        "{:#?}",
        rows(&report)
    );
}

/// ⑥ A `with` clause written by the part joins the element's supertype face:
/// `D().m()` resolves through the augmented mixin application.
///
/// Oracle: `class D {}` in the library, `augment class D with M {}` plus
/// `mixin M { void m() {} }` in the part, `D().m();` — no issues.
#[test]
fn part_with_clause_joins_the_supertype_face() {
    let report = augment_part(
        "class D {}\n\
         void callIt() {\n  D().m();\n}\n",
        "augment class D with M {}\n\
         mixin M {\n  void m() {}\n}\n",
    );
    assert_no_undefined(&report);
}

/// ⑦ The merge widens the face to the element's declarations, never to
/// "anything resolves": a name declared in neither unit still reports, once.
///
/// Oracle: `nowhere;` in the library with only `baz` added by the part
/// reports exactly one `undefined_identifier`.
#[test]
fn a_name_in_neither_unit_still_reports() {
    let report = augment_part(
        "class C {}\n\
         void test() {\n  nowhere;\n}\n",
        "augment class C {\n  static const baz = 1;\n}\n",
    );
    let reported = rows(&report);
    assert!(
        reported
            .iter()
            .any(|row| row.contains("undefined_identifier") && row.contains("'nowhere'")),
        "{:#?}",
        reported
    );
    assert_eq!(
        code_names(&report)
            .iter()
            .filter(|name| name.starts_with("undefined_"))
            .count(),
        1,
        "{:#?}",
        reported
    );
}

/// ⑧ The introduction's own constructor still answers: an element whose
/// introduction declares `C()` keeps it even when a part adds a named one.
///
/// Oracle: `class C { C(); }` in the library plus `augment class C {
/// C.named(); }` in the part, with `C();` and `C.named();` — no issues. This
/// is the arm a naive "any fragment declares a constructor" rule breaks.
#[test]
fn part_fragment_keeps_an_introduction_declared_default() {
    let report = augment_part(
        "class C {\n  C();\n}\n\
         void make() {\n  C();\n  C.named();\n}\n",
        "augment class C {\n  C.named();\n}\n",
    );
    assert_no_undefined(&report);
}
