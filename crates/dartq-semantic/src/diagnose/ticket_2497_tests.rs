//! The #2497 pins: the extension-applicability bound check
//! (`row_resolution`'s `arg_applicable` — `applicable_extensions.dart`
//! `applicableTo`'s instantiated-bound gate). An `on` argument spelling
//! an applied face's own type parameter instantiates against the
//! receiver's argument, and the instantiation must satisfy the
//! parameter's declared bound or the face never applies and the access
//! keeps its `undefined_*` face. An in-scope type-parameter argument
//! answers through its OWN bound (`subtype.dart`'s `TypeParameterType`
//! arm): `T extends Enum` satisfies `Enum` while the unbounded `T` reads
//! `Object?` and the concrete bound rejects it — across the
//! `List`-to-`Iterable` subtype hop exactly as across the same head.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2245_tests` carries the same shape).
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// Same-head `Iterable<T>`: the bounded parameter resolves the face
/// silently; the unbounded parameter's implicit `Object?` bound violates
/// `T extends Enum` and `byName` reports `undefined_method` — the
/// oracle's verdict on both rows.
#[test]
fn same_head_parameter_bound_decides() {
    let bounded = report(
        "class Enum {} class Iterable<T> {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         T pick<T extends Enum>(Iterable<T> values) { values.byName(); return values.first; }",
    );
    assert!(
        code_names(&bounded).is_empty(),
        "{:?}",
        code_names(&bounded)
    );
    let unbounded = report(
        "class Enum {} class Iterable<T> {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         T pickUnbounded<T>(Iterable<T> values) { values.byName(); return values.first; }",
    );
    assert_eq!(
        code_names(&unbounded),
        ["undefined_method"],
        "{:?}",
        code_names(&unbounded)
    );
}

/// The subtype hop (`List` reaches `Iterable` by fiat): the `on`
/// parameter binds the receiver's same-position argument through the
/// identity mapping every SDK collection edge writes
/// (`List<E> implements Iterable<E>`), and the bound check answers the
/// same verdict it would on the same head — bounded silent, unbounded
/// `undefined_method` (the #2488 witness shape, `values.byName` over
/// `List<T>`).
#[test]
fn subtype_hop_parameter_bound_decides() {
    let bounded = report(
        "class Enum {} class Iterable<T> {} class List<T> {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         T pick<T extends Enum>(List<T> values) { values.byName(); return values.first; }",
    );
    assert!(
        code_names(&bounded).is_empty(),
        "{:?}",
        code_names(&bounded)
    );
    let unbounded = report(
        "class Enum {} class Iterable<T> {} class List<T> {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         T pickUnbounded<T>(List<T> values) { values.byName(); return values.first; }",
    );
    assert_eq!(
        code_names(&unbounded),
        ["undefined_method"],
        "{:?}",
        code_names(&unbounded)
    );
}

/// Concrete instantiations ride the same check across the hop:
/// `Iterable<String>` violates `T extends Enum` and reports, while an
/// enum argument (`Color`, whose declared kind carries the synthetic
/// `Enum` edge) satisfies it silently — the `Color.values.byName` census
/// row the #2488 activation keyed on.
#[test]
fn concrete_argument_bound_decides() {
    let violates = report(
        "class Enum {} class Iterable<T> {} class String {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         void f(Iterable<String> values) { values.byName(); }",
    );
    assert_eq!(
        code_names(&violates),
        ["undefined_method"],
        "{:?}",
        code_names(&violates)
    );
    let satisfies = report(
        "class Enum {} class Iterable<T> {} enum Color { red } \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         void f(Iterable<Color> values) { values.byName(); }",
    );
    assert!(
        code_names(&satisfies).is_empty(),
        "{:?}",
        code_names(&satisfies)
    );
}
