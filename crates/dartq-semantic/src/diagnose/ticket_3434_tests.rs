//! The #3434 pins — an `augment` fragment's members join the merged element
//! face of the declaration it augments.
//!
//! # The defect
//!
//! Upstream resolves every identifier in a member body against the MERGED
//! namespace of the element (`Augmentations/scoping_A01_t01`: "The static
//! and instance member namespaces for an augmented type or extension
//! declaration include the declarations of all members in the introductory
//! and augmenting declarations"). The collector kept the fragment's face
//! apart from the introduction's whenever both lived in ONE unit: the
//! augment's `members` were dropped at its close, so `class C { void t() {
//! this.qux; } } augment class C { int qux = 1; }` reported
//! `undefined_getter`, an augmented enum constant was invisible to `E.e0`
//! and to the switch witness set, and an augmentation's constructor,
//! `with`/`implements` edges and implicit-default suppression were all lost.
//! `collector.rs` now folds the fragment into the element row at its close
//! (`Builder::fold_fragment`) instead of dropping it, so every consumer
//! (member lookup, the override and unused faces, the library-wide fragment
//! merge) reads one merged face.
//!
//! # The rig
//!
//! These are SINGLE-UNIT fixtures (the introduction and its `augment` heads
//! in one file), so they drive `report_ops_experiments_with` — the
//! `report_ops` fold with the corpus's `enable-experiment` opt-ins.
//! Augmentations are off by default even at the rig's language version
//! (`dartq-language/data/experimental_features.yaml` has no `enabledIn` for
//! the flag), so the experiment is required for the `augment` keyword to
//! parse as an augmentation at all.
//!
//! Unlike `report_ops`, this entry point mounts **no `CORE_PRELUDE`**: the
//! only core spellings resolvable are the [`super::test_rig`] stub names
//! (`Object`, `Never`, `Null`, `Function`, …). A fixture here must therefore
//! declare every class it names and cannot use `String`/`int`/`num` — a pin
//! that does reports `undefined_class` on its own fixture text (offsets into
//! the fixture, no prelude offset), which says nothing about the merged
//! face.
//!
//! Every expectation below was checked against the oracle
//! (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze` over a package with
//! `analysis_options.yaml` `analyzer: enable-experiment: [augmentations]`);
//! each pin names the oracle verdict it encodes.

use super::test_rig::{code_names, report_ops, report_ops_experiments_with, rows};

/// One single-unit fixture with the `augmentations` experiment enabled.
fn augmentations(source: &str) -> super::FileReport {
    report_ops_experiments_with(source, &[], None, &[dartq_language::Feature::Augmentations])
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

/// ① A fragment's field and method are the element's: the introduction's
/// body reads both through `this`.
///
/// Oracle: `Object readIt() => this.qux; Object callIt() => this.m();` over
/// `augment class C { Object qux = 0; Object m() => 0; }` — no issues
/// (`scoping_A01_t01`'s "Checks that it is possible to use `this.name` …
/// for referencing declarations in another introduction or augmenting
/// declaration"). `Object` is a `CORE_STUB_NAMES` entry; the fixture cannot
/// name `String`/`int` because `report_ops_experiments_with` mounts no
/// `CORE_PRELUDE` (see the rig note above).
#[test]
fn fragment_field_and_method_reach_the_intro_body() {
    let report = augmentations(
        "class C {\n  \
           Object readIt() => this.qux;\n  \
           Object callIt() => this.m();\n\
         }\n\
         augment class C {\n  \
           Object qux = 0;\n  \
           Object m() => 0;\n\
         }\n",
    );
    assert_no_undefined(&report);
}

/// ② A fragment's static member answers a BARE read in the introduction's
/// body (the implicit-`this` / lexical class scope serves statics too).
///
/// Oracle: `class C { void test() { baz; } } augment class C { static const
/// baz = 2; }` — no issues.
#[test]
fn fragment_static_member_answers_a_bare_read() {
    let report = augmentations(
        "class C {\n  void test() {\n    baz;\n  }\n}\n\
         augment class C {\n  static const baz = 2;\n}\n",
    );
    assert_no_undefined(&report);
}

/// ③ An augmented enum's constants join the enum face: `E.e0` and `F.f1`
/// resolve, `F.f0` (the introduction's own) still does.
///
/// Oracle: `enum E {} augment enum E { e0; }` plus `enum F { f0; }
/// augment enum F { f1; }` read through `E.e0`/`F.f0`/`F.f1` — no issues
/// (`enums_A01_t01`: "Checks that enums with no members are allowed").
#[test]
fn fragment_enum_constants_join_the_enum_face() {
    let report = augmentations(
        "enum E {}\n\
         augment enum E { e0; }\n\
         enum F { f0; }\n\
         augment enum F { f1; }\n\
         void main() {\n  \
           Object? a = E.e0;\n  \
           Object? b = F.f0;\n  \
           Object? c = F.f1;\n  \
           a;\n  b;\n  c;\n\
         }\n",
    );
    assert_no_undefined(&report);
}

/// ④ The fragment's constants join the enum's WITNESS set: a switch over an
/// augmented enum that covers only the introduction's constant is
/// non-exhaustive, and the report names the fragment's constant.
///
/// Oracle: `enum E { e0; } augment enum E { e1; }` with `switch (e) { case
/// E.e0: }` reports `non_exhaustive_switch_statement` at the switch, "since
/// it doesn't match the pattern 'E.e1'". This pin fails unless
/// `Container::enum_constants` (not just `members`) carries the fragment's
/// constant.
#[test]
fn fragment_enum_constant_joins_the_witness_set() {
    let report = augmentations(
        "enum E { e0; }\n\
         augment enum E { e1; }\n\
         void test(E e) {\n  \
           switch (e) {\n    \
             case E.e0:\n  \
           }\n\
         }\n",
    );
    assert!(
        code_names(&report).contains(&"non_exhaustive_switch_statement"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑤ An augmentation's constructor is the element's constructor: `C.named()`
/// declared only in the fragment is callable.
///
/// Oracle: `class C { C() {} } augment class C { C.named() {} }` with
/// `C.named();` — no issues.
#[test]
fn fragment_constructor_joins_the_constructor_face() {
    let report = augmentations(
        "class C {\n  C() {}\n}\n\
         augment class C {\n  C.named() {}\n}\n\
         void main() {\n  C.named();\n}\n",
    );
    assert_no_undefined(&report);
}

/// ⑥ A named constructor declared only in a fragment kills the IMPLICIT
/// default of the introduction: `class C {}` plus `augment class C {
/// C.named(); }` leaves `C()` undefined.
///
/// Oracle: that shape reports `new_with_undefined_constructor_default`,
/// "The class 'C' doesn't have an unnamed constructor". The flag is re-derived
/// from the ELEMENT's summed constructor declarations
/// (`Builder::element_ctors`), not from either fragment alone.
#[test]
fn fragment_named_constructor_kills_the_implicit_default() {
    let report = augmentations(
        "class C {}\n\
         augment class C {\n  C.named();\n}\n\
         void main() {\n  C();\n}\n",
    );
    assert!(
        code_names(&report).contains(&"new_with_undefined_constructor_default"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑦ A `with` clause written by the fragment joins the element's supertype
/// face: `C().m()` resolves through the augmented mixin application.
///
/// Oracle: `mixin M { void m() {} } class C {} augment class C with M {}`
/// with `C().m();` — no issues.
#[test]
fn fragment_with_clause_joins_the_supertype_face() {
    let report = augmentations(
        "mixin M {\n  void m() {}\n}\n\
         class C {}\n\
         augment class C with M {}\n\
         void main() {\n  C().m();\n}\n",
    );
    assert_no_undefined(&report);
}

/// ⑧ The merge widens the face to the element's members, never to "anything
/// resolves": a name declared in neither fragment still reports.
///
/// Oracle: `class C { void test() { absent; } } augment class C { static
/// const baz = 2; }` reports `undefined_identifier` on `absent` (and nothing
/// else).
#[test]
fn a_member_in_neither_fragment_still_reports() {
    let report = augmentations(
        "class C {\n  void test() {\n    absent;\n  }\n}\n\
         augment class C {\n  static const baz = 2;\n}\n",
    );
    let reported = rows(&report);
    assert!(
        reported
            .iter()
            .any(|row| row.contains("undefined_identifier") && row.contains("'absent'")),
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

/// ⑨ Two fragments of one element fold in declaration order, whichever
/// declaration opened the element row: `enum E {}` plus two `augment enum E`
/// heads keeps `e0` then `e1`, both readable.
///
/// Oracle: `enum E {} augment enum E { e0; } augment enum E { e1, }` (the
/// trailing-comma shape `enums_A01_t01` pins) with `E.e0`/`E.e1` reads — no
/// issues; the oracle's `constants` list is `[e0, e1]` in that order.
#[test]
fn two_fragments_fold_in_declaration_order() {
    let report = augmentations(
        "enum E {}\n\
         augment enum E { e0; }\n\
         augment enum E { e1, }\n\
         void main() {\n  \
           Object? a = E.e0;\n  \
           Object? b = E.e1;\n  \
           a;\n  b;\n\
         }\n",
    );
    assert_no_undefined(&report);
}

/// ⑩ The merged face serves the OVERRIDE check, not just plain lookup: a
/// fragment member that satisfies an inherited signature badly reports
/// `invalid_override` exactly as an introduction-declared one does.
///
/// Oracle: `class Base { A m() => A(); } class D extends Base {}` plus
/// `augment class D { @override B m() => B(); }` (both classes declared in
/// the fixture) reports `invalid_override` at the fragment's `m` ("'D.m'
/// ('B Function()') isn't a valid override of 'Base.m' ('A Function()')").
/// Fails while the fragment's member is absent from the resolved container's
/// member map.
#[test]
fn fragment_member_reaches_the_override_face() {
    let report = augmentations(
        "class A {}\n\
         class B {}\n\
         class Base {\n  A m() => A();\n}\n\
         class D extends Base {}\n\
         augment class D {\n  \
           @override\n  \
           B m() => B();\n\
         }\n",
    );
    assert!(
        code_names(&report).contains(&"invalid_override"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑪ Plain (non-augmenting) declarations keep their own faces: the fold is
/// scoped to `augment` heads, so an ordinary class whose members read `this`
/// still resolves them from its own row.
///
/// Oracle: `class C { int a = 1; int readA() => this.a; }` — no issues.
#[test]
fn plain_introductions_keep_their_own_face() {
    let report = report_ops(
        "class C {\n  \
           int a = 1;\n  \
           int readA() => this.a;\n\
         }\n",
    );
    assert_no_undefined(&report);
}
