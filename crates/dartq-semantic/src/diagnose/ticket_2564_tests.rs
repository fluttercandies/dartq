//! The #2564 pins: the nesting-safe creation-head latch (the census's
//! analyzer format.dart `vTableGet` x8 family).
//!
//! The engine's `pending_bare_creation` slot is single, so a keyword
//! creation whose argument list contains its own invocation
//! (`const Reader<W>(const Inner())`) had the slot overwritten by the
//! inner head at the inner open and consumed by the inner close's
//! `report_bare_creation_miss` path — the outer unnamed-creation fold
//! never fired, the argument rows stood above the stack, and the member
//! merge popped the inner head's row as the receiver (`undefined_method`
//! naming `_Inner` where the oracle answers on `Reader<W>` and stays
//! silent). The latch now rides the argument level: each close re-arms
//! its own open's head, the fold drains the group's rows, and the created
//! type — instantiated with the written type arguments — owns every
//! downstream member face.
//!
//! The rig declares no imports, so every pin spells custom types only
//! (`ticket_2472_tests` carries the same rule — the harness's bare world
//! seeds no core spellings, keywords excepted).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2472_tests` carries the same shape).
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

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

const READERS: &str = "\
class Reader<T> {
  const Reader(Inner reader);
  Out<T> select(Out<T> def) => def;
}

class Inner {
  const Inner();
}

class Out<T> {
  const Out();
}

class Wrap<T> {
  const Wrap(Reader<T> reader);
  T pick(T def) => def;
}

class Apple {
  const Apple();
}
";

/// The census's `??=` receiver shape: the keyword creation with written
/// type arguments and a nested creation argument folds to a receiver
/// typed `Reader<Apple>` — `select` resolves there and the oracle stays
/// silent. The pre-fix model lost the creation-head latch to the inner
/// `const Inner()` group and reported `undefined_method` naming `Inner`.
#[test]
fn const_creation_with_nested_argument_folds_created_type() {
    let source = format!(
        "{READERS}\
class Holder {{
  Apple? staged;
  Out<Apple> get ready {{
    return staged ??= const Reader<Apple>(
      const Inner(),
    ).select(const Out<Apple>());
  }}
}}
"
    );
    let result = report(&source);
    assert_eq!(count_code(&result, "undefined_method"), 0, "{:?}", messages(&result));
    assert_eq!(count_code(&result, "undefined_getter"), 0, "{:?}", messages(&result));
}

/// A genuine miss on the created type reports against the constructor's
/// type — the fold's receiver owns the verdict, never the nested
/// argument's head.
#[test]
fn const_creation_miss_names_created_type() {
    let source = format!(
        "{READERS}\
class Holder {{
  Apple? staged;
  Out<Apple> get ready {{
    return staged ??= const Reader<Apple>(
      const Inner(),
    ).absent(const Out<Apple>());
  }}
}}
"
    );
    let result = report(&source);
    assert_eq!(count_code(&result, "undefined_method"), 1, "{:?}", messages(&result));
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code.name == "undefined_method" && d.message.contains("Reader")),
        "{:?}",
        messages(&result)
    );
}

/// Two stacked creation groups: the outer keyword creation's fold fires
/// with its own head re-armed after the inner group's open/close cycle —
/// `pick` resolves on `Wrap<Apple>`, silent.
#[test]
fn const_creation_survives_nested_creation_argument() {
    let source = format!(
        "{READERS}\
class Holder {{
  Apple get ready {{
    return const Wrap<Apple>(
      const Reader<Apple>(const Inner()),
    ).pick(const Apple());
  }}
}}
"
    );
    let result = report(&source);
    assert_eq!(count_code(&result, "undefined_method"), 0, "{:?}", messages(&result));
}

/// The implicit (no-keyword) creation with written type arguments keeps
/// its callee-transport path — the latch re-arm must not disturb it.
#[test]
fn implicit_creation_with_type_arguments_still_resolves() {
    let source = format!(
        "{READERS}\
class Holder {{
  Out<Apple> get ready {{
    return Reader<Apple>(const Inner()).select(const Out<Apple>());
  }}
}}
"
    );
    let result = report(&source);
    assert_eq!(count_code(&result, "undefined_method"), 0, "{:?}", messages(&result));
}

fn messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

/// Diagnoses `main` against a one-import library under `prefix`
/// (`ticket_2432_tests`'s rig shape).
fn report_prefixed(main_source: &str, lib_source: &str, prefix: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_lib = files.intern(Path::new("lib.dart"));
    let scan = |source: &str, file: FileId| {
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
        scanned
    };
    let scanned_main = scan(main_source, file_main);
    let scanned_lib = scan(lib_source, file_lib);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main_source, &scanned_main.tokens, features, &strings);
    let elements_lib = collect(lib_source, &scanned_lib.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_lib = builder.intern("package:t/lib.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_lib, elements_lib.clone());
    for (name, decl) in elements_main.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    for (name, decl) in elements_lib.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_lib, name, file_lib);
    }
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_lib),
            prefix: Some(strings.intern(prefix)),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    // The bare twin (`import 'lib.dart';` beside the prefixed one): the
    // miss pin needs a decidable member verdict, and the prefix-only face
    // under-reports through the bare scope — the rig imports both so the
    // fold's Created receiver resolves.
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main_source,
        &scanned_main.tokens,
        features,
        &strings,
        file_main,
        lib_main,
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

/// The census's exact receiver spelling: a prefixed head with written
/// type arguments and a nested creation argument in an expression
/// context (`??=`). The type-argument head detector's no-selector guard
/// rejected the dot-preceded head outright, so no chain answered and the
/// member merge read the nested argument's head — the oracle answers on
/// the prefixed class and stays silent. The nested argument and the
/// default spell main-side classes (the import is prefix-only, `Seed` is
/// invisible bare), and `dynamic` needs no core table.
#[test]
fn prefixed_const_creation_with_type_arguments_folds_created_type() {
    let lib = "class ListReader<T> {\n\
               const ListReader(dynamic reader);\n\
               T vTableGet(T def) => def;\n\
               }\n";
    let main = "import 'lib.dart' as fb;\n\
                class Marker { const Marker(); }\n\
                class Seed { const Seed(); }\n\
                class Holder {\n\
                Marker? staged;\n\
                Marker get ready {\n\
                return staged ??= const fb.ListReader<Marker>(\n\
                const Seed(),\n\
                ).vTableGet(const Marker());\n\
                }\n\
                }\n";
    let rep = report_prefixed(main, lib, "fb");
    assert_eq!(count_code(&rep, "undefined_method"), 0, "{:?}", messages(&rep));
    assert_eq!(count_code(&rep, "undefined_getter"), 0, "{:?}", messages(&rep));
}

/// A miss through the same prefixed spelling reports against the
/// prefixed class's bare name — the fold's receiver owns the verdict.
#[test]
fn prefixed_const_creation_miss_still_reports() {
    let lib = "class ListReader<T> {\n\
               const ListReader(dynamic reader);\n\
               T vTableGet(T def) => def;\n\
               }\n";
    let main = "import 'lib.dart' as fb;\n\
                class Marker { const Marker(); }\n\
                class Seed { const Seed(); }\n\
                class Holder {\n\
                Marker? staged;\n\
                Marker get ready {\n\
                return staged ??= const fb.ListReader<Marker>(\n\
                const Seed(),\n\
                ).absent(const Marker());\n\
                }\n\
                }\n";
    let rep = report_prefixed(main, lib, "fb");
    assert_eq!(count_code(&rep, "undefined_method"), 1, "{:?}", messages(&rep));
    assert!(
        rep.diagnostics
            .iter()
            .any(|d| d.code.name == "undefined_method" && d.message.contains("ListReader")),
        "{:?}",
        messages(&rep)
    );
}
