//! #3578 — the four augmentation diagnostics whose emit sites needed new
//! collector facts: the `function_not_complete_after_augmentations` /
//! `induced_{getter,setter}_not_complete_after_augmentations` TOP-LEVEL arms
//! (upstream `_checkForFunctionBodyCompleteness` /
//! `_checkForIncompleteInducedAccessors` on a `FunctionDeclaration` /
//! `TopLevelVariableDeclaration`, error_verifier.dart:5256-5321 /
//! :5415-5456), the `_checkForConstVariableAugmentation` pair
//! (`:4241-4280`), and
//! `default_value_already_specified_in_augmentation_chain` (`:4346-4375`).
//!
//! The member arms landed with #3569; these pins cover the newly reachable
//! faces. Every expectation below is a verbatim oracle 3.13.7 shape —
//! the anchor comments in
//! `pkg/analyzer/test/src/diagnostics/const_variable_augmentation_test.dart`
//! mark the variable's name token for both const codes, and
//! `default_value_already_specified_in_augmentation_chain` reports at the
//! `=`/`:` separator.
//!
//! ```text
//! const int foo = 0; + augment abstract final int foo; -> AUGMENTS_CONSTANT_VARIABLE @ aug `foo`
//! const int foo = 0; + augment int get foo;            -> AUGMENTS_CONSTANT_VARIABLE @ aug `foo`
//! int foo;           + augment const int foo = 0;      -> CONSTANT_VARIABLE_AUGMENTATION @ aug `foo`
//! augment enum E { augment foo }                       -> CONSTANT_VARIABLE_AUGMENTATION @ `foo`
//! void f([int a = 0]); + augment void f([int a = 1]);  -> DEFAULT_VALUE_..._IN_AUGMENTATION_CHAIN @ `=`
//! void f();          + augment void f();               -> FUNCTION_NOT_COMPLETE_AFTER_AUGMENTATIONS @ intro `f`
//! abstract int foo;  + augment abstract int foo;       -> INDUCED_GETTER/SETTER_NOT_COMPLETE @ intro `foo`
//! ```

use crate::diagnose::test_rig::{
    CORE_PRELUDE, report_ops_augment_experiments, report_ops_augment_intro_experiments,
    report_ops_experiments_with,
};
use crate::FileReport;
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

/// The two-unit rig, experiment on, reading the AUGMENTING unit's report
/// (the const/duplication rows anchor on the augmenting file).
fn with_augment(main: &str, augment: &str) -> FileReport {
    report_ops_augment_experiments(main, augment, &[Feature::Augmentations])
}

/// The two-unit rig, experiment on, reading the INTRODUCING unit's report
/// (the intro-side completeness rows anchor on the declaring file).
fn intro_report(main: &str, augment: &str) -> FileReport {
    report_ops_augment_intro_experiments(main, augment, &[Feature::Augmentations])
}

/// A fixture offset into the AUGMENT unit's own source (no prelude there —
/// the rig prepends `CORE_PRELUDE` to the intro unit only).
fn aug_at(augment: &str, needle: &str) -> u32 {
    augment.find(needle).expect("anchor") as u32
}

/// A fixture offset into the INTRO unit's source, shifted by the rig's
/// one-line prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("anchor")) as u32
}

/// The debug list on failure (code, span, message).
fn dump(report: &FileReport) -> Vec<(&'static str, u32, u32, &str)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length, d.message.as_str()))
        .collect()
}

const ACV: &str = "augments_constant_variable";
const CVA: &str = "constant_variable_augmentation";
const DVS: &str = "default_value_already_specified_in_augmentation_chain";
const FNC: &str = "function_not_complete_after_augmentations";
const IGET: &str = "induced_getter_not_complete_after_augmentations";
const ISET: &str = "induced_setter_not_complete_after_augmentations";

// ── augments_constant_variable ────────────────────────────────────────────

/// `const int foo = 0;` + a non-const `augment` variable: the fragment is
/// an augmentation, itself not const, but the element's first fragment is
/// const — `augments_constant_variable` at the augment's name token
/// (upstream `test_topLevelVariable_augmentedByVariable`).
#[test]
fn augments_constant_variable_by_top_level_variable() {
    let main = "const int foo = 0;\n";
    let aug = "augment abstract final int foo;\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, ACV),
        vec![(aug_at(aug, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == ACV)
        .map(|d| d.message.to_string())
        .expect("row");
    assert_eq!(message, "Const variables can't be augmented.");
}

/// The accessor arm: an `augment int get foo` over a const variable reads
/// the variable element's firstFragment.const — same row, same anchor
/// (upstream `test_topLevelVariable_augmentedByGetter`).
#[test]
fn augments_constant_variable_by_top_level_getter() {
    let main = "const int foo = 0;\n";
    let aug = "augment int get foo;\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, ACV),
        vec![(aug_at(aug, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// The setter arm over a const variable (upstream
/// `test_topLevelVariable_augmentedBySetter`).
#[test]
fn augments_constant_variable_by_top_level_setter() {
    let main = "const int foo = 0;\n";
    let aug = "augment void set foo(int _);\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, ACV),
        vec![(aug_at(aug, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// A static class field target — the member arm reads
/// `MemberEntry::is_const` (upstream `test_class_staticField_augmentedByField`).
#[test]
fn augments_constant_variable_static_field() {
    let main = "class A {\n  static const int foo = 0;\n}\n";
    let aug = "augment class A {\n  augment static abstract final int foo;\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, ACV),
        vec![(aug_at(aug, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// A non-const intro never fires the row — the silent twin.
#[test]
fn augments_constant_variable_non_const_intro_is_silent() {
    let main = "final int foo = 0;\n";
    let aug = "augment abstract final int foo;\n";
    let report = with_augment(main, aug);
    assert!(rows(&report, ACV).is_empty(), "{:?}", dump(&report));
}

// ── constant_variable_augmentation ────────────────────────────────────────

/// A `const` variable augmentation reports its own arm —
/// `constant_variable_augmentation` at the augment's name (upstream
/// `test_topLevelFinalVariable_augmentedByVariable`).
#[test]
fn constant_variable_augmentation_top_level() {
    let main = "final int foo = 0;\n";
    let aug = "augment const int foo = 0;\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, CVA),
        vec![(aug_at(aug, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == CVA)
        .map(|d| d.message.to_string())
        .expect("row");
    assert_eq!(message, "Variable augmentations can't be const.");
}

/// The member arm: `augment static const int foo` inside an augment class
/// (upstream `test_class_staticGetter_augmentedByField`).
#[test]
fn constant_variable_augmentation_static_field() {
    let main = "class A {\n  static int get foo => 0;\n}\n";
    let aug = "augment class A {\n  augment static const int foo = 0;\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, CVA),
        vec![(aug_at(aug, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// An enum constant carrying its own `augment` keyword is an augmentation
/// fragment with `isConst = true` unconditionally — the fragment arm fires
/// even when the constant's name resolves nothing (upstream
/// `test_enum_nothing_augmentedByEnumConstant`).
#[test]
fn constant_variable_augmentation_enum_constant() {
    let main = "enum E {\n  bar\n}\n";
    let aug = "augment enum E {\n  augment foo,\n}\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, CVA),
        vec![(aug_at(aug, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// A non-const variable augmentation never fires — the silent twin.
#[test]
fn constant_variable_augmentation_non_const_is_silent() {
    let main = "final int foo = 0;\n";
    let aug = "augment abstract final int foo;\n";
    let report = with_augment(main, aug);
    assert!(rows(&report, CVA).is_empty(), "{:?}", dump(&report));
}

/// An enum constant without the `augment` keyword is a plain introduction —
/// the arm stays silent (the intro's own `isConst` is not the gate).
#[test]
fn enum_constant_without_augment_is_silent() {
    let main = "enum E {\n  foo\n}\n";
    let report = report_ops_experiments_with(
        &format!("{CORE_PRELUDE}{main}"),
        &[],
        None,
        &[Feature::Augmentations],
    );
    assert!(rows(&report, CVA).is_empty(), "{:?}", dump(&report));
}

// ── default_value_already_specified_in_augmentation_chain ─────────────────

/// The augment's optional-positional default re-specifies the intro's —
/// the row anchors at the augment's own `=` separator (upstream
/// `defaultClause.separator`).
#[test]
fn default_value_already_specified_positional() {
    let main = "void f([int a = 0]);\n";
    let aug = "augment void f([int a = 1]);\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, DVS),
        vec![(aug_at(aug, "="), 1)],
        "{:?}",
        dump(&report)
    );
    let message = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == DVS)
        .map(|d| d.message.to_string())
        .expect("row");
    assert_eq!(
        message,
        "The default value for this optional parameter was already specified in the augmentation chain."
    );
}

/// The named arm pairs by name (upstream the same `precedingFragments`
/// walk on the named parameter element).
#[test]
fn default_value_already_specified_named() {
    let main = "void f({int a = 0});\n";
    let aug = "augment void f({int a = 1});\n";
    let report = with_augment(main, aug);
    assert_eq!(
        rows(&report, DVS),
        vec![(aug_at(aug, "="), 1)],
        "{:?}",
        dump(&report)
    );
}

/// No preceding default — a first-time default on the augment stays silent.
#[test]
fn default_value_first_time_is_silent() {
    let main = "void f([int a = 0]);\n";
    let aug = "augment void f([int a = 1, int b = 2]);\n";
    let report = with_augment(main, aug);
    // `a` re-specifies (fires), `b`'s default is new (silent) — the row
    // count is exactly one, anchored at `a`'s separator.
    assert_eq!(
        rows(&report, DVS),
        vec![(aug_at(aug, "="), 1)],
        "{:?}",
        dump(&report)
    );
}

// ── function_not_complete_after_augmentations — top-level arm ─────────────

/// A top-level `void f();` intro plus a body-less `augment void f();` —
/// two incomplete fragments report at the intro's name token (the member
/// face's #3569 walk, extended to the declaration-table rows).
#[test]
fn function_not_complete_top_level_function() {
    let main = "void foo();\n";
    let aug = "augment void foo();\n";
    let report = intro_report(main, aug);
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

/// The top-level getter arm — the accessor chain collects a top-level
/// variable augment's induced getter too.
#[test]
fn function_not_complete_top_level_getter() {
    let main = "int get foo;\n";
    let aug = "augment int get foo;\n";
    let report = intro_report(main, aug);
    assert_eq!(
        rows(&report, FNC),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
}

/// A completing `augment` finishes the chain — no row.
#[test]
fn function_not_complete_top_level_completing_augment_is_silent() {
    let main = "void foo();\n";
    let aug = "augment void foo() {}\n";
    let report = intro_report(main, aug);
    assert!(rows(&report, FNC).is_empty(), "{:?}", dump(&report));
}

// ── induced_{getter,setter}_not_complete_after_augmentations — top-level ──

/// `abstract int foo;` at top level plus an equally abstract augment: both
/// induced chains grew and stay incomplete — the pair reports at the
/// variable's name token.
#[test]
fn induced_accessors_not_complete_top_level_abstract() {
    let main = "abstract int foo;\n";
    let aug = "augment abstract int foo;\n";
    let report = intro_report(main, aug);
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
}

/// A `final` abstract top-level variable induces no setter — the getter
/// row alone fires.
#[test]
fn induced_getter_only_for_final_top_level_variable() {
    let main = "abstract final int foo;\n";
    let aug = "augment abstract final int foo;\n";
    let report = intro_report(main, aug);
    assert_eq!(
        rows(&report, IGET),
        vec![(at(main, "foo"), 3)],
        "{:?}",
        dump(&report)
    );
    assert!(rows(&report, ISET).is_empty(), "{:?}", dump(&report));
}

/// A non-abstract top-level variable is a complete induced accessor — the
/// walk never reaches the face.
#[test]
fn non_abstract_top_level_variable_is_silent() {
    let main = "int foo = 0;\n";
    let aug = "augment int foo = 1;\n";
    let report = intro_report(main, aug);
    assert!(rows(&report, IGET).is_empty(), "{:?}", dump(&report));
    assert!(rows(&report, ISET).is_empty(), "{:?}", dump(&report));
}
