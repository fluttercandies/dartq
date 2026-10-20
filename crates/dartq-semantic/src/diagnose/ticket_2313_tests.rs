//! The #2313 differential pins: a supertype climb's queued `implements`
//! faces resolve through the library of the file whose clause spells them,
//! not through whatever library the drain last climbed into. The burn-down
//! shape (drift's `versioned_schema.dart`/`subquery.dart` false
//! `override_on_non_overriding_member` rows): `implements A, B` keeps both
//! faces independently — the first face's climb must not strand the second
//! face's spelling in a foreign scope. Each fixture was probed against
//! oracle `dart analyze` (fvm default) via the isolated `mixinrepro` tree.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The minimal import closure: `main.dart` imports `mix.dart`, whose mixin
/// implements one interface from `dialect.dart` and one from `api.dart`.
/// Diagnoses `main.dart` and returns its report. The concrete classes are
/// abstract-incomplete by shape (the oracle's
/// `non_abstract_class_inherits_abstract_member` row stands in every
/// fixture); only the `@override` faces vary.
fn report(main: &str, mix: &str) -> FileReport {
    let dialect = "abstract interface class ResultSetDsl { void dslThing(); }";
    let api = "abstract interface class Entity { String get entityName; }";
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_mix = files.intern(Path::new("mix.dart"));
    let file_api = files.intern(Path::new("api.dart"));
    let file_dialect = files.intern(Path::new("dialect.dart"));
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
    let scanned_main = scan(main, file_main);
    let scanned_mix = scan(mix, file_mix);
    let scanned_api = scan(api, file_api);
    let scanned_dialect = scan(dialect, file_dialect);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = |source: &str, scanned: &dartq_scanner::ScanResult| {
        collect(source, &scanned.tokens, features, &strings)
    };
    let elements_main = elements(main, &scanned_main);
    let elements_mix = elements(mix, &scanned_mix);
    let elements_api = elements(api, &scanned_api);
    let elements_dialect = elements(dialect, &scanned_dialect);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_mix = builder.intern("package:t/mix.dart");
    let lib_api = builder.intern("package:t/api.dart");
    let lib_dialect = builder.intern("package:t/dialect.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_mix, elements_mix.clone());
    index.insert(file_api, elements_api.clone());
    index.insert(file_dialect, elements_dialect.clone());
    for (lib, file, elements) in [
        (lib_main, file_main, &elements_main),
        (lib_mix, file_mix, &elements_mix),
        (lib_api, file_api, &elements_api),
        (lib_dialect, file_dialect, &elements_dialect),
    ] {
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
    }
    let edge = |target| ImportEdge {
        target: Some(target),
        prefix: None,
        deferred: false,
        combinators: Combinators::unfiltered(),
    };
    builder.import(lib_mix, edge(lib_dialect));
    builder.import(lib_mix, edge(lib_api));
    builder.import(lib_main, edge(lib_mix));
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main,
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The mixin declaring the two-interface merge (the `mixinrepro`
/// `mix.dart`, minus the third import whose `Unrelated` member type no
/// single-fold rig can model).
const MIX: &str = "import 'dialect.dart';\n\
                   import 'api.dart';\n\
                   mixin Res implements ResultSetDsl, Entity {\n\
                     String? get alias;\n\
                   }";
/// The member the second face (`Entity`) alone carries.
/// `class String {}` is the harness's core stand-in (#537): the rig has
/// no SDK, so main's `String` type uses need a declaration to stay off
/// the undefined-class rows.
const MAIN: &str = "import 'mix.dart';\n\
                    class String {}\n\
                    final class C with Res {\n\
                      @override\n\
                      final String entityName = 'x';\n\
                    }";
/// The one legitimate row every fixture below carries: `C` never
/// implements `ResultSetDsl.dslThing` (the oracle reports it too).
const INCOMPLETE: &str = "non_abstract_class_inherits_abstract_member";

/// The burn-down shape: the first face (`ResultSetDsl`) dead-ends with no
/// superclass, and the drained second face (`Entity`) resolves in the
/// writer's library — the `@override entityName` spares.
#[test]
fn second_interface_face_survives_the_first_dead_end() {
    let rep = report(MAIN, MIX);
    assert_eq!(code_names(&rep), [INCOMPLETE], "{:?}", code_names(&rep));
}

/// The declaration-order mirror: with the member-carrying face first, the
/// drain order swaps — the member answers at the first hop, and the
/// second face's climb stays put.
#[test]
fn swapped_interface_order_stays_silent_on_the_override() {
    let mix = "import 'dialect.dart';\n\
               import 'api.dart';\n\
               mixin Res implements Entity, ResultSetDsl {\n\
                 String? get alias;\n\
               }";
    let rep = report(MAIN, mix);
    assert_eq!(code_names(&rep), [INCOMPLETE], "{:?}", code_names(&rep));
}

/// A plain class's `extends` chain into the two-interface merge (drift
/// `subquery.dart:59`): the merge is not a mixin face — the abstract
/// intermediate's `implements` list behaves identically through the
/// superclass hop.
#[test]
fn plain_class_through_an_abstract_intermediate_keeps_both_faces() {
    let mix = "import 'dialect.dart';\n\
               import 'api.dart';\n\
               abstract class Base implements ResultSetDsl, Entity {\n\
                 String? get alias;\n\
               }";
    let main = "import 'mix.dart';\n\
                class String {}\n\
                final class C extends Base {\n\
                  @override\n\
                  final String entityName = 'x';\n\
                  @override\n\
                  String? get alias => null;\n\
                }";
    let rep = report(main, mix);
    assert_eq!(code_names(&rep), [INCOMPLETE], "{:?}", code_names(&rep));
}

/// The anti-vacuity control: with the sibling face gone, the same
/// `@override entityName` must report — the fixtures above pin a real
/// resolution, not a suppressed checker.
#[test]
fn missing_sibling_face_still_reports_the_non_override() {
    let mix = "import 'dialect.dart';\n\
               mixin Res implements ResultSetDsl {\n\
                 String? get alias;\n\
               }";
    let rep = report(MAIN, mix);
    assert_eq!(
        code_names(&rep),
        [INCOMPLETE, "override_on_non_overriding_member"],
        "{:?}",
        code_names(&rep)
    );
}
