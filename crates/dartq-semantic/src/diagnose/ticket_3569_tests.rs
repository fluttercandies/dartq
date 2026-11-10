//! #3569 — `function_not_complete_after_augmentations` and the
//! `induced_{getter,setter}_not_complete_after_augmentations` pair
//! (upstream `_checkForFunctionBodyCompleteness`,
//! error_verifier.dart:5256-5321, and
//! `_checkForIncompleteInducedAccessors`, :5415-5456).
//!
//! Both are intro-side walks under the `augmentations` experiment: a chain
//! of more than one fragment with no complete fragment reports at the
//! introductory declaration's name token. The member faces are the
//! implemented arms (the model carries no name span or completeness bits
//! for the top-level intros). Every expectation below is a verbatim oracle
//! 3.13.7 shape (`diagnostics/executable_body/ExecutableBodyTest` in the
//! conformance corpus):
//!
//! ```text
//! class A { static int get foo; }         + augment no-body get  -> FUNCTION_NOT_COMPLETE_AFTER_AUGMENTATIONS @ `foo` (3)
//! class A { static void foo(); }          + augment no-body m    -> FUNCTION_NOT_COMPLETE_AFTER_AUGMENTATIONS @ `foo` (3)
//! class A { static set foo(v); }          + augment no-body set  -> FUNCTION_NOT_COMPLETE_AFTER_AUGMENTATIONS @ `foo` (3)
//! class A { static abstract int foo; }    + augment abstract     -> INDUCED_GETTER/SETTER_NOT_COMPLETE @ `foo` (3)
//! class A { static abstract final int f;}+ augment abstract     -> INDUCED_GETTER_NOT_COMPLETE only @ `f` (1)
//! singleton incomplete intro                                       -> silent (other faces own it)
//! completing augment                                               -> silent
//! ```

use crate::FileReport;
use crate::diagnose::test_rig::{
    CORE_PRELUDE, report_ops_augment_intro_experiments, report_ops_experiments_with,
};
use dartq_language::Feature;

/// The fixture's rows of one code as `(offset, length)`, ascending.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut out = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect::<Vec<_>>();
    out.sort_unstable();
    out
}

/// The two-unit rig, experiment on, reading the INTRODUCING unit's report.
fn with_augment(main: &str, augment: &str) -> FileReport {
    report_ops_augment_intro_experiments(main, augment, &[Feature::Augmentations])
}

/// A fixture offset, shifted by the rig's one-line prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("anchor")) as u32
}

/// The debug list on failure (code, span, message).
fn dump(report: &FileReport) -> Vec<(&'static str, u32, u32, &str)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.as_str(),
            )
        })
        .collect()
}

const FNC: &str = "function_not_complete_after_augmentations";
const IGET: &str = "induced_getter_not_complete_after_augmentations";
const ISET: &str = "induced_setter_not_complete_after_augmentations";

// ── function_not_complete_after_augmentations ────────────────────────────

/// A static method intro plus a body-less `augment` — two incomplete
/// fragments — reports at the intro's name.
#[test]
fn function_not_complete_static_method() {
    let main = "class A {\n  static void foo();\n}\n";
    let aug = "augment class A {\n  augment static void foo();\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, FNC),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == FNC)
        .map(|d| d.message.to_string())
        .expect("row");
    assert!(
        message.contains("'foo'") && message.ends_with("augmentations are applied."),
        "{message}"
    );
}

/// The static getter arm — same chain shape through the accessor element.
#[test]
fn function_not_complete_static_getter() {
    let main = "class A {\n  static int get foo;\n}\n";
    let aug = "augment class A {\n  augment static int get foo;\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, FNC),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// The static setter arm — the member's `foo=` lookup spelling folds to the
/// property name for both the chain key and the message.
#[test]
fn function_not_complete_static_setter() {
    let main = "class A {\n  static set foo(v);\n}\n";
    let aug = "augment class A {\n  augment static set foo(v);\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, FNC),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == FNC)
        .map(|d| d.message.to_string())
        .expect("row");
    assert!(message.contains("'foo'"), "{message}");
}

/// Extension instance members stay in the face (upstream only exempts the
/// instance members of a class, enum, or mixin).
#[test]
fn function_not_complete_extension_instance_method() {
    let main = "extension E on int {\n  void foo();\n}\n";
    let aug = "augment extension E {\n  augment void foo();\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, FNC),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// A class instance member never reports here — the interface-wide faces
/// (`concrete_class_with_abstract_member` and kin) own that shape.
#[test]
fn instance_method_of_class_is_silent() {
    let main = "abstract class A {\n  void foo();\n}\n";
    let aug = "augment class A {\n  augment void foo();\n}\n";
    let report = with_augment(main, aug);
    assert!(rows(&report, FNC).is_empty(), "{:?}", dump(&report));
}

/// An `augment` that supplies a body completes the chain — no row.
#[test]
fn completing_augment_is_silent() {
    let main = "class A {\n  static void foo();\n}\n";
    let aug = "augment class A {\n  augment static void foo() {}\n}\n";
    let report = with_augment(main, aug);
    assert!(rows(&report, FNC).is_empty(), "{:?}", dump(&report));
}

/// A singleton incomplete intro is `missing_function_body`'s territory —
/// this code never fires on a one-fragment chain.
#[test]
fn singleton_incomplete_method_is_silent() {
    let main = "class A {\n  static void foo();\n}\n";
    let report = report_ops_experiments_with(
        &format!("{CORE_PRELUDE}{main}"),
        &[],
        None,
        &[Feature::Augmentations],
    );
    assert!(rows(&report, FNC).is_empty(), "{:?}", dump(&report));
}

/// Without the experiment the row never fires.
#[test]
fn needs_the_augmentations_experiment() {
    let main = "class A {\n  static void foo();\n}\n";
    let aug = "augment class A {\n  augment static void foo();\n}\n";
    let report = report_ops_augment_intro_experiments(main, aug, &[]);
    assert!(rows(&report, FNC).is_empty(), "{:?}", dump(&report));
}

// ── induced_{getter,setter}_not_complete_after_augmentations ─────────────

/// `static abstract int foo;` plus an equally incomplete abstract augment:
/// both induced chains grew and stay incomplete — the pair reports at the
/// variable's name.
#[test]
fn induced_accessors_not_complete_static_abstract_field() {
    let main = "abstract class A {\n  static abstract int foo;\n}\n";
    let aug = "augment class A {\n  augment static abstract int foo;\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, IGET),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    assert_eq!(
        rows(&report, ISET),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    let getter_message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == IGET)
        .map(|d| d.message.to_string())
        .expect("getter row");
    assert!(
        getter_message.contains("'foo'") && getter_message.ends_with("augmentations are applied."),
        "{getter_message}"
    );
    let setter_message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == ISET)
        .map(|d| d.message.to_string())
        .expect("setter row");
    assert!(
        setter_message.starts_with("The setter induced by 'foo'"),
        "{setter_message}"
    );
}

/// A `final` field induces no setter — only the getter row fires.
#[test]
fn induced_getter_only_for_final_field() {
    let main = "abstract class A {\n  static abstract final int foo;\n}\n";
    let aug = "augment class A {\n  augment static abstract final int foo;\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, IGET),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    assert!(rows(&report, ISET).is_empty(), "{:?}", dump(&report));
}

/// A non-abstract augment's induced accessors are complete — the chain
/// completes and both rows stay silent.
#[test]
fn completing_field_augment_is_silent() {
    let main = "abstract class A {\n  static abstract int foo;\n}\n";
    let aug = "augment class A {\n  augment static int foo = 0;\n}\n";
    let report = with_augment(main, aug);
    assert!(rows(&report, IGET).is_empty(), "{:?}", dump(&report));
    assert!(rows(&report, ISET).is_empty(), "{:?}", dump(&report));
}

/// A singleton abstract field reports the `*_without_body` sibling codes
/// (another face's rows) — never this pair.
#[test]
fn singleton_abstract_field_is_silent() {
    let main = "abstract class A {\n  static abstract int foo;\n}\n";
    let report = report_ops_experiments_with(
        &format!("{CORE_PRELUDE}{main}"),
        &[],
        None,
        &[Feature::Augmentations],
    );
    assert!(rows(&report, IGET).is_empty(), "{:?}", dump(&report));
    assert!(rows(&report, ISET).is_empty(), "{:?}", dump(&report));
}
