//! #3556 — canonical-type comparison for the augmentation shape checks
//! (upstream `error_verifier.dart`'s `_checkForAugmentation*` formal/
//! return/bound walks judge `DartType` with `==`, which is the canonical
//! `InterfaceType`/`FunctionType`/`TypeParameterType` equality — typedef
//! aliases expand and import prefixes resolve to the declaring element
//! before the comparison — while the message text keeps the written
//! `getDisplayString` spelling). Before this fix the rows judged the
//! captured `Ty` nominally, so an alias spelling vs its expansion and a
//! `p.C` vs `C` spelling each fabricated a mismatch; the preceding-
//! fragment chain (a member checked against the fragment BEFORE its own,
//! not only the introduction) was missing entirely.
//!
//! Oracle 3.13.7 rows, probed in a package carrying
//! `enable-experiment: [augmentations]`:
//!
//! ```text
//! typedef IntAlias = int; void f(IntAlias x);   + augment void f(int x);    silent
//! typedef IntAlias = int; void f(int x);        + augment void f(IntAlias x); silent
//! void f(int x);                                + augment void f(String x);
//!   augmentation_formal_parameter_type_mismatch @ `String`
//! void f();                                     + augment IntAlias f() {}
//!   augmentation_return_type_mismatch @ `IntAlias` ('IntAlias' vs 'void')
//! import C as C and as p.C; void f(C x);        + augment void f(p.C x);    silent
//! class C { C(int _); } augment C { augment C(int p2); } augment C { augment C(int p3); }
//!   augmentation_positional_formal_parameter_name @ `p3` (names 'p2')
//! ```

use crate::FileReport;
use crate::diagnose::test_rig::{
    RigImport, code_names, report_ops, report_ops_augment, report_ops_imports, rows,
};

/// The report for a same-unit intro + augment pair (the single-file shape
/// of the a/b pins — `augment` in the unit under test).
fn same_unit(source: &str) -> FileReport {
    report_ops(source)
}

/// The rows of one code as `offset`s, for the anchor assertions.
fn at(report: &FileReport, code: &str) -> Vec<u32> {
    let mut out = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    out.sort_unstable();
    out
}

/// The single message a row carries, verbatim.
fn message(report: &FileReport, code: &str) -> String {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .unwrap_or_default()
}

// —— (a) typedef alias vs its expansion — silent both directions ————————

/// The augment writes the expansion where the declaration wrote the
/// alias: `f(IntAlias)` vs `f(int)` judges equal upstream (the typedef
/// expands before `==`), so the oracle prints no row.
#[test]
fn aliased_formal_then_expanded_augment_is_silent() {
    let report = same_unit(
        "typedef IntAlias = int;\n\
         void f(IntAlias x);\n\
         augment void f(int x);\n",
    );
    assert!(
        !code_names(&report).contains(&"augmentation_formal_parameter_type_mismatch"),
        "{:#?}",
        rows(&report)
    );
}

/// The other direction — the declaration writes the expansion, the
/// augment writes the alias — is the same judged type and stays silent.
#[test]
fn expanded_formal_then_aliased_augment_is_silent() {
    let report = same_unit(
        "typedef IntAlias = int;\n\
         void f(int x);\n\
         augment void f(IntAlias x);\n",
    );
    assert!(
        !code_names(&report).contains(&"augmentation_formal_parameter_type_mismatch"),
        "{:#?}",
        rows(&report)
    );
}

// —— (b) genuinely different types — the row still fires ————————————————

/// `int` vs `String` are different canonical types: the row fires and the
/// message carries the WRITTEN spellings (`String` then `int`), the
/// written/judged split — the judged side canonicalizes, the reported
/// side renders what the author wrote.
#[test]
fn a_different_formal_type_reports_with_the_written_spellings() {
    let report = same_unit("void f(int x);\naugment void f(String x);\n");
    assert_eq!(
        at(&report, "augmentation_formal_parameter_type_mismatch").len(),
        1,
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        message(&report, "augmentation_formal_parameter_type_mismatch"),
        "The augmentation's formal parameter type 'String' must be the same as \
         the declaration's formal parameter type 'int'."
    );
}

/// The return-type arm: `IntAlias` expands to `int`, so `void` vs
/// `IntAlias` still mismatch — but the message keeps the WRITTEN
/// `IntAlias` spelling (upstream `getDisplayString` writes the alias's
/// own name), verbatim oracle text.
#[test]
fn an_aliased_return_type_reports_with_its_written_spelling() {
    let report = same_unit(
        "typedef IntAlias = int;\n\
         void f();\n\
         augment IntAlias f() {}\n",
    );
    assert_eq!(
        message(&report, "augmentation_return_type_mismatch"),
        "The augmentation's return type 'IntAlias' must be the same as \
         the introductory declaration's return type 'void'."
    );
}

// —— (c) import-prefixed spelling — silent ——————————————————————————————

/// `p.C` and `C` resolve to the one declaring element upstream, so a
/// declaration spelling `C` and an augmentation spelling `p.C` judge
/// equal — the prefix is an artifact of the import spelling, not of the
/// type.
#[test]
fn a_prefixed_augment_spelling_matches_the_plain_declaration() {
    let report = report_ops_imports(
        "import 'package:p/c.dart';\n\
         import 'package:p/c.dart' as p;\n\
         void f(C x);\n\
         augment void f(p.C x);\n",
        &[
            RigImport {
                uri: "package:p/c.dart",
                prefix: None,
                source: "class C {}\n",
            },
            RigImport {
                uri: "package:p/c.dart",
                prefix: Some("p"),
                source: "class C {}\n",
            },
        ],
    );
    assert!(
        !code_names(&report).contains(&"augmentation_formal_parameter_type_mismatch"),
        "{:#?}",
        rows(&report)
    );
}

// —— (d) the preceding-fragment chain ———————————————————————————————————

/// Three fragments — `C(int _)`, `augment C(int p2)`, `augment C(int p3)`
/// — check each positional name against the fragment BEFORE its own, not
/// only the introduction: `p3` must match `p2` (the preceding augment's
/// spelling, upstream `currentParameter.precedingFragments`), not the
/// introduction's `_`. The oracle reports once, at `p3`, naming `p2`.
#[test]
fn the_last_fragment_judges_against_the_preceding_augment_not_the_intro() {
    let report = report_ops_augment(
        "class C { C(int _); }\n",
        "augment class C { augment C(int p2); }\n\
         augment class C { augment C(int p3); }\n",
    );
    assert_eq!(
        message(&report, "augmentation_positional_formal_parameter_name"),
        "The parameter name 'p3' must either match the name 'p2' from a \
         preceding declaration or be '_'."
    );
    let mut anchors = at(&report, "augmentation_positional_formal_parameter_name");
    anchors.dedup();
    assert_eq!(anchors.len(), 1, "{:#?}", rows(&report));
}
