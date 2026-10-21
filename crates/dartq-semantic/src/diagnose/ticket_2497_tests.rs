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

/// Same-head `Iterable<T>`: the bounded parameter resolves `byName`
/// through the extension silently while the local `Iterable` (shadowing
/// dart:core's — the auto-import never sees the local spelling) declares
/// no `first`, so the oracle fires `undefined_getter` on the read alone;
/// the unbounded parameter's implicit `Object?` bound violates
/// `T extends Enum` and `byName` reports `undefined_method` beside it.
/// KNOWN DIVERGENCE (1 corpus row, analyzer): the SDK face table also
/// spells an `EnumByName on Iterable<T extends Enum>` whose
/// spelling-level applicability cannot see that the receiver's head is
/// the LOCAL `Iterable`, so dartq adds an
/// `ambiguous_extension_member_access` row beside the oracle's set — the
/// full equality re-arms when the SDK faces gain a receiver-container
/// provenance gate.
#[test]
fn same_head_parameter_bound_decides() {
    let bounded = report(
        "class Enum {} class Iterable<T> {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         T pick<T extends Enum>(Iterable<T> values) { values.byName(); return values.first; }",
    );
    let codes = code_names(&bounded);
    assert!(
        codes.contains(&"undefined_getter") && !codes.contains(&"undefined_method"),
        "{codes:?}"
    );
    let unbounded = report(
        "class Enum {} class Iterable<T> {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         T pickUnbounded<T>(Iterable<T> values) { values.byName(); return values.first; }",
    );
    assert_eq!(
        code_names(&unbounded),
        ["undefined_method", "undefined_getter"],
        "{:?}",
        code_names(&unbounded)
    );
}

/// The subtype hop in the no-core rig universe: the fiat collection edge
/// (`List<E> implements Iterable<E>`) carries the bound check when the
/// parameter satisfies it (`byName` silent) and the local `List`'s own
/// missing `first` reports alone; the unbounded parameter's bound
/// violation reports `byName` beside it. KNOWN UNIVERSE SPLIT: under the
/// live oracle (dart:core auto-imported) the fixture's LOCAL `List`
/// declares no edge to its LOCAL `Iterable`, so `byName` reports
/// `undefined_method` in BOTH polarities — the CLI run matches that; the
/// receiver-container provenance gate that reconciles the two universes
/// is the same parked face as the same-head divergence above.
#[test]
fn subtype_hop_parameter_bound_decides() {
    let bounded = report(
        "class Enum {} class Iterable<T> {} class List<T> {} \
         extension EnumByName<T extends Enum> on Iterable<T> { void byName() {} } \
         T pick<T extends Enum>(List<T> values) { values.byName(); return values.first; }",
    );
    assert_eq!(
        code_names(&bounded),
        ["undefined_getter"],
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
        ["undefined_method", "undefined_getter"],
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
