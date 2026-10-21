//! The #2426 pins: the closure-formal inference's receiver-head gate — the
//! unindexed receiver's first type argument stands in for `map`'s `E` only
//! when the receiver's written head is provably a type (an invalid head is
//! the oracle's `InvalidType`, which has no function type to substitute
//! into, so the formal keeps `dynamic` and its member reads stay silent),
//! and the `switch (this)` scrutinee carries the staged static type of
//! `this` — an exhaustive enum switch in an enum's own method closes, and
//! the false `body_might_complete_normally` the unknown scrutinee reported
//! disappears.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library — the same
/// minimal rig `ticket_2324_tests` carries.
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

fn has_code(report: &FileReport, code: &str) -> bool {
    report.diagnostics.iter().any(|d| d.code.name == code)
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// An invalid receiver head never arms the closure formal's inference: the
/// `NodeList<Annotation>` formal of an unresolvable spelling is the
/// oracle's `InvalidType` — no function type answers the literal's schema,
/// the formal keeps `dynamic`, and `annotation.foo` stays silent (probe
/// r-nodelist). The gate that keeps the head's own `undefined_class` rows
/// from growing member verdicts is the same one the oracle applies.
#[test]
fn invalid_receiver_head_keeps_the_closure_formal_dynamic() {
    let source = "void validate(NodeList<Annotation> annotations) { \
                  var packed = annotations.where((annotation) => annotation.foo).toList(); \
                  print(packed); \
                  }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "undefined_getter"), "{:?}", code_names(&rep));
    assert!(!has_code(&rep, "undefined_method"), "{:?}", code_names(&rep));
}

/// A provable receiver head keeps the stand-in: `List<Und>`'s first written
/// argument is `map`'s `E` schema, `Und` is not a type, and
/// `greatestClosure` binds the formal `Object?` (probe d2a) — the member
/// read inside the literal reports the shared unchecked code.
#[test]
fn valid_receiver_head_still_binds_the_object_question_recovery() {
    let source = "class List<T> {} \
                  void f(List<Und> xs) { \
                  xs.map((e) => e.isPacked).toList(); \
                  }";
    let rep = report(source);
    assert!(
        has_code(&rep, "unchecked_use_of_nullable_value"),
        "{:?}",
        code_names(&rep)
    );
}

/// `switch (this)` in an enum's own method: the staged static type of
/// `this` (the same answer the receiver push reads — the container's name)
/// gives the scrutinee the real enum type, `is_always_exhaustive` closes
/// the switch over every case returning, and the body never completes
/// normally — the false row the unknown scrutinee used to report is gone
/// (probe d3b/d3d's exhaustive-over-`this` face).
#[test]
fn exhaustive_switch_over_this_closes_the_body() {
    let source = "enum Direction { north, south; \
                  Direction reverse() { \
                  switch (this) { \
                  case Direction.north: return Direction.south; \
                  case Direction.south: return Direction.north; \
                  } \
                  } \
                  }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "body_might_complete_normally"),
        "{:?}",
        code_names(&rep)
    );
}
