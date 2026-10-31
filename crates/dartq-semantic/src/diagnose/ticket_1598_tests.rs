//! The #1598 pins (burn-wave ruling sheet N5): the acceptance rows whose
//! emitters landed through #1462 (F-L constant-pattern const-ness), #1550
//! (the issue63883 capture face) and the generic-call inference chain.
//!
//! The ticket's rows are `constant_pattern_with_non_constant_expression` and
//! `invocation_of_non_function_expression` on the 70d226a5 constant-pattern
//! case, and `could_not_infer` on the 5adcb8ee (issue63883) unsolved-parameter
//! call. Each is pinned on exactly one face so a later flip — a fabricated
//! generic row beside an answered resolution row, or a double-report from the
//! creation bound-seed seam — fails here first.
//!
//! Upstream law (analyzer 3.13.7, the pinned oracle version):
//! `pkg/analyzer/lib/src/dart/element/generic_inferrer.dart:365`
//! (`tryChooseFinalTypes` per-parameter report) and `:506`
//! (`_checkArgumentsNotMatchingBounds`), reported at the call's `errorEntity`
//! and naming the type parameter; `pkg/analyzer/lib/src/dart/constant/
//! constant_verifier.dart` `visitConstantPattern` → `_evaluateAndReportError`
//! for the pattern expression's own verdict.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports — the
/// sibling lane rig mirrored (each lane owns its copy).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// 5adcb8ee (issue63883): the outer call's formal `C<List<T>>` solves `X` to
/// `List<Object?>` inside the creation while the literal writes
/// `void Function(List<int>)` — the parameter is unsatisfied at the call, so
/// `could_not_infer` names `X` anchored on the creation's callee.
#[test]
fn could_not_infer_names_the_unsolved_parameter_at_the_call() {
    let source = "class C<X> {\n  C(void Function(X) x);\n}\n\
                  T check<T>(C<List<T>> f) => throw 0;\n\
                  void main() {\n  var x = check(C((List<int> x) {}));\n  print(x);\n}\n";
    let report = report(source);
    let at = source.find("C((List<int> x) {})").expect("anchor") as u32;
    assert_eq!(
        rows(&report, "could_not_infer"),
        vec![(at, 1)],
        "{:?}",
        report.diagnostics
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "could_not_infer")
        .expect("row");
    assert!(
        row.message
            .starts_with("Couldn't infer type parameter 'X'."),
        "{}",
        row.message
    );
    assert!(
        row.message
            .contains("Tried to infer 'List<Object?>' for 'X' which doesn't work:"),
        "{}",
        row.message
    );
}

/// 70d226a5: `case this():` runs the send machinery inside the pattern window
/// — `Class` declares `call`, so the pattern's own verdict is `NotConstant`
/// (the generic row) — while `super()` resolves against `Object`, whose
/// non-callable report already answers the expression
/// (`invocation_of_non_function_expression`), so the generic row is
/// suppressed there. One row per face, no cross-face duplication.
#[test]
fn const_pattern_this_super_keeps_one_row_per_face() {
    let source = "class Class { const Class([a]); call() {} \
                  test(o) { switch (o) { case this(): break; case super(): break; } } }";
    let report = report(source);
    let this_at = source.find("this()").expect("this anchor") as u32;
    let super_at = source.find("super()").expect("super anchor") as u32;
    assert_eq!(
        rows(&report, "constant_pattern_with_non_constant_expression"),
        vec![(this_at, 6)],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        rows(&report, "invocation_of_non_function_expression"),
        vec![(super_at, 5)],
        "{:?}",
        report.diagnostics
    );
}

/// The creation bound-seed seam and the call-side solve walk the same
/// extends clause for a bare creation: `B<X extends B<X>>`'s `B()` must not
/// double-report (the call-side suppress arm keeps the seam's single row).
#[test]
fn bare_creation_does_not_double_report_could_not_infer() {
    let source = "class B<X extends B<X>> {\n  B();\n}\nvoid main() {\n  B();\n}\n";
    let report = report(source);
    let found = rows(&report, "could_not_infer");
    assert!(
        found.len() <= 1,
        "double report: {found:?} in {:?}",
        report.diagnostics
    );
    for diagnostic in report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "could_not_infer")
    {
        assert!(
            diagnostic
                .message
                .starts_with("Couldn't infer type parameter 'X'."),
            "{}",
            diagnostic.message
        );
    }
}
