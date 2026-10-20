//! The #2416 pins: the record literal's named labels ride the record type.
//!
//! #2406 (`1e4d909c`) folded const-literal payloads through the same
//! `record_literal_names` stack the row-shape reader (`record_field_tys`)
//! consults — `end_record_literal` popped the labels for the payload twin,
//! leaving the row path with an empty list, so `(entryIndex: 1, header: null)`
//! produced a positional-only `(int, Null)` record instead of the written
//! `({int entryIndex, Null? header})`. Every named-literal use then
//! mismatched its declared record shape: `return (…)` against a named
//! return type fired `return_of_invalid_type` (21 rows in the analyzer
//! corpus alone), `_loading = (progress: progress);` against `({num?
//! progress})` fired `field_initializer_not_assignable`, and the riverpod /
//! archive twins carried the same face.
//!
//! The fix pops the label entry exactly once — `end_record_literal` hands
//! the same `names` list to the payload twin (which keeps its own copy for
//! duplicate comparison) and to `record_field_tys` (which folds the named
//! split into the record row's `ty`). The const-duplicate semantics stay
//! untouched: the payload still compares by value.
//!
//! Oracle provenance: `/tmp/probe2416/record/a.dart` reports zero rows on
//! `dart analyze` 3.13.1; `/tmp/census7_target/release/dartq` emitted
//! `return_of_invalid_type` at both return sites pre-fix.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of an import-free library — the
/// single-file rig mirrored from the sibling ticket suites.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    builder.seed_implicit_core(
        lib,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map",
        ],
    );
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The corpus exemplar (`analyzer/lib/src/dart/analysis/
/// single_file_byte_store.dart:654`): `return (entryIndex: 1, header:
/// null);` against the declared `({int entryIndex, String? header})`
/// return type. Pre-fix the row typed `(int, Null)` and the return face
/// reported `return_of_invalid_type`.
#[test]
fn named_record_return_stays_silent() {
    let source = "({int entryIndex, String? header}) f(bool c, String? h) {\n\
                  \x20 if (c) {\n\
                  \x20   return (entryIndex: 1, header: h);\n\
                  \x20 }\n\
                  \x20 return (entryIndex: -1, header: null);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The mixed positional+named shape (`analyzer`'s
/// `type_constraint_gatherer.dart:305`): `return (P_instantiated,
/// Q_instantiated, typeParametersToEliminate: newTypeParameters);`
/// against `(TypeImpl, TypeImpl, {List<TypeParameterElementImpl>
/// typeParametersToEliminate})` — the positional split must feed the
/// record's positional fields while the trailing named pair lands on
/// `typeParametersToEliminate`.
#[test]
fn positional_and_named_record_return_stays_silent() {
    let source = "(int, int, {String? note}) f(int a, int b) {\n\
                  \x20 return (a, b, note: 'x');\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The field-initializer twin (`riverpod/lib/src/core/async_value.dart:
/// 741`): `_loading = (progress: progress);` against a field typed
/// `({num? progress})`. Pre-fix the initializer row typed `(num?,)` and
/// the assignability face reported `field_initializer_not_assignable`.
#[test]
fn named_record_field_initializer_stays_silent() {
    let source = "class A {\n\
                  \x20 final ({num? progress}) loading;\n\
                  \x20 A({num? progress}) : loading = (progress: progress);\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The const-literal twin keeps its payload semantics — the duplicate
/// check still fires on structurally-equal named records (the #2406
/// contract: two `(Namespaces.html, 'tag')` literals are equal and must
/// report `equal_elements_in_const_set`). The named split now feeds BOTH
/// faces without stealing the row's label list.
#[test]
fn const_named_record_duplicates_still_report() {
    let source = "void f() {\n\
                  \x20 const { (a: 1, b: 'x'), (a: 1, b: 'x') };\n\
                  }\n";
    assert_eq!(
        code_names(&report(source)),
        ["equal_elements_in_const_set"]
    );
}

/// A nested record literal keeps its own label list — the outer literal's
/// names stack must not leak into the inner one's field split. The oracle
/// types `(1, (s: 'x'))` as `(int, ({String s}))`; pre-fix the inner
/// `(s: 'x')` popped its own entry clean but left the outer literal to
/// consume a stale empty list, flipping its named split silent.
#[test]
fn nested_named_record_keeps_outer_labels() {
    let source = "({int head, ({String inner}) nested}) f() {\n\
                  \x20 return (head: 1, nested: (inner: 'x'));\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}
