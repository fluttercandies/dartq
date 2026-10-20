//! The #2406 pins: the `equal_elements_in_const_set` /
//! `equal_keys_in_const_map` equality face over structural values. The
//! capture's const literals now fold to payload-carrying values (lists,
//! sets, maps, records by their element/field values, symbols by name —
//! the upstream `DartObjectImpl` state equality), so the duplicate twins
//! compare by value and never by opaque kind: the html package's
//! `(namespace, tag)` record sets (`constants.dart`) were 103 false rows
//! because every record folded to the same opaque instance.
//!
//! Oracle provenance: every fixture probed against `dart analyze` 3.13.1
//! (`/tmp/probe2406/probe.dart`) — records differing in any field value
//! stay silent, genuinely equal records report once at the later
//! element, named-label order never decides, and the created-instance
//! corner keeps its documented deferral.

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

fn count(report: &FileReport, name: &str) -> usize {
    code_names(report)
        .into_iter()
        .filter(|c| *c == name)
        .count()
}

/// The html shape (the 103-row census cluster): record elements whose
/// field values differ are distinct constants — the oracle stays silent
/// on every `constants.dart` set, so any row here is the over-report.
#[test]
fn record_field_differences_stay_silent() {
    let rep = report(
        "const ns = 'html';\n\
         const scoping = {(ns, 'applet'), (ns, 'caption'), (ns, 'marquee')};\n\
         const formatting = {(ns, 'a'), (ns, 'b'), (ns, 'i')};",
    );
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
}

/// Genuinely equal records report once, at the later element's own span.
#[test]
fn equal_records_report_once() {
    let source = "const s = {(1, 2), (1, 2)};";
    let rep = report(source);
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
    let dup = rep
        .diagnostics
        .iter()
        .find(|d| d.code.name == "equal_elements_in_const_set")
        .expect("the duplicate row");
    // The second element's span — the literal's value push anchors at its
    // close token (the shared literal-push posture), so the row lands at
    // or after the second element's `(`.
    let second = source.rfind("(1, 2)").expect("second element");
    assert!(
        dup.range.offset >= second as u32,
        "row at {}: must anchor in the second element (from {second})",
        dup.range.offset
    );
}

/// Named labels join the shape: same labels same values report, differing
/// labels stay silent, and written label order never decides (the
/// upstream `RecordState` compares the named map).
#[test]
fn named_record_labels_decide() {
    // Same labels, same values: report.
    let rep = report("const s = {(a: 1), (a: 1)};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
    // Differing labels, same values: distinct shapes — silent.
    let rep = report("const s = {(a: 1, b: 2), (a: 1, c: 2)};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
    // Swapped written order, same labels: equal — report.
    let rep = report("const s = {(a: 1, b: 2), (b: 2, a: 1)};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
    // Mixed positional + named, identical: report.
    let rep = report("const s = {(1, a: 2), (1, a: 2)};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
}

/// Record shape includes arity: a one-field and a two-field record are
/// never equal.
#[test]
fn record_arity_differences_stay_silent() {
    let rep = report("const s = {(1, 2), (1,)};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
}

/// Nested const collections compare by element values too — the
/// literal-as-element push carries the payload (the official
/// `test_const_list_hasEqual`/`noEqual` pair).
#[test]
fn nested_collections_compare_by_values() {
    // Equal lists: report.
    let rep = report("const s = {[0], [0]};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
    // Differing lists: silent.
    let rep = report("const s = {[0], [1]};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
    // Differing nested sets and maps: silent.
    let rep = report("const s = { {1}, {2} };");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
    let rep = report("const s = { {1: 2}, {1: 3} };");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
}

/// Symbols compare by name (the upstream `SymbolState`).
#[test]
fn symbols_compare_by_name() {
    let rep = report("const s = {#a, #a};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
    let rep = report("const s = {#a, #b};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
}

/// The already-aligned value kinds keep their faces: strings, enum
/// constants, and type literals.
#[test]
fn primitive_enum_and_type_faces_hold() {
    let rep = report("const s = {'a', 'a'};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
    let rep = report("enum E { a, b }\nconst s = {E.a, E.a};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
    let rep = report("enum E { a, b }\nconst s = {E.a, E.b};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 0, "{rep:?}");
    let rep = report("const s = {int, int};");
    assert_eq!(count(&rep, "equal_elements_in_const_set"), 1, "{rep:?}");
}

/// The map twin rides the same face: record keys equal by value report;
/// differing keys stay silent.
#[test]
fn map_keys_compare_by_value() {
    let rep = report("const m = {(1, 2): 'x', (1, 2): 'y'};");
    assert_eq!(count(&rep, "equal_keys_in_const_map"), 1, "{rep:?}");
    let rep = report("const m = {[0]: 1, [1]: 2};");
    assert_eq!(count(&rep, "equal_keys_in_const_map"), 0, "{rep:?}");
}
