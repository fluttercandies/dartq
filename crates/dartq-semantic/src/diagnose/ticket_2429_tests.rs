//! The #2429 pins: a prefixed supertype-clause face (`implements idl.X`)
//! binds through the prefix's export scope instead of dying in the bare
//! scope.
//!
//! The element model keeps only the clause continuation (`implements idl.X`
//! records `X` — the collector's bare-name convention), so every face walk
//! over a prefixed clause died at the bare-scope miss: the @override gate
//! found nothing through the prefixed interface and reported
//! `override_on_non_overriding_*` for members the interface does declare
//! (~118 dartq-only OVERRIDE_ON_NON_OVERRIDING_MEMBER rows on the analyzer
//! corpus, the `idl.*` prefixed-import shape). The fix recovers the dotted
//! spellings from the diagnosing file's header tokens and binds a prefixed
//! face through [`prefixed_container`] — the same container identity an
//! unprefixed import of the same library yields — in the member walk, the
//! face-list climb, and the @override gate alike; a prefixed spelling the
//! prefix table cannot answer keeps the dead-edge verdict (#2420's arms).
//!
//! Oracle provenance: probed against `dart analyze` (Dart 3.13.1, fvm
//! 3.47.1) under `/tmp/probe2429` — `valid` (a concrete `@override void
//! hello()` over `implements idl.X` is silent, rc 0), `invalid`
//! (`@override void absent()` reports OVERRIDE_ON_NON_OVERRIDING_MEMBER
//! exactly), `deadprefix` (`implements idl.Missing` keeps the override
//! report beside the type-row error).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `main_source` as `main.dart` importing `lib.dart` — prefixed
/// (`import 'lib.dart' as idl;` + a prefixed graph edge) when `prefixed`,
/// bare otherwise. The library declarations seed both libraries like the
/// two-file corpus shape (#2420's rig); the member runs sit in `main.dart`.
fn report_with(main_source: &str, lib_source: &str, prefixed: bool) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_lib = files.intern(Path::new("lib.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    if prefixed {
        // Two prefixed edges of the same library (`idl` and `alt`) — the
        // per-container keying pins use both spellings.
        for prefix in ["idl", "alt"] {
            builder.import(
                lib_main,
                ImportEdge {
                    target: Some(lib_lib),
                    prefix: Some(strings.intern(prefix)),
                    deferred: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
    } else {
        builder.import(
            lib_main,
            ImportEdge {
                target: Some(lib_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    builder.seed_implicit_core(
        lib_main,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "Iterable", "void",
        ],
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

fn has(names: &[&'static str], code: &str) -> bool {
    names.contains(&code)
}

const IFACE_LIB: &str = "abstract class X {\n\
                        \x20 void hello();\n\
                        }\n";

/// The valid prefixed shape is silent (the `valid` probe's rc-0 verdict): a
/// member the prefixed interface declares spares the @override gate, and a
/// member read resolves through the prefixed face in the member walk.
#[test]
fn prefixed_implements_declared_member_stays_silent() {
    let main_source = "import 'lib.dart' as idl;\n\
                       \n\
                       class C implements idl.X {\n\
                       \x20 @override\n\
                       \x20 void hello() {}\n\
                       }\n\
                       \n\
                       void f(C c) {\n\
                       \x20 c.hello();\n\
                       }\n";
    let names = code_names(&report_with(main_source, IFACE_LIB, true));
    assert!(
        !has(&names, "override_on_non_overriding_member"),
        "hello is declared on the prefixed interface: {names:?}"
    );
    assert!(
        !has(&names, "undefined_method")
            && !has(&names, "undefined_getter")
            && !has(&names, "undefined_identifier"),
        "the prefixed face's members resolve: {names:?}"
    );
}

/// The invalid shape reports exactly like the oracle (`invalid` probe): an
/// @override member the prefixed interface does not declare keeps
/// `override_on_non_overriding_member` once the face binds.
#[test]
fn prefixed_implements_missing_override_reports() {
    let main_source = "import 'lib.dart' as idl;\n\
                       \n\
                       class C implements idl.X {\n\
                       \x20 void hello() {}\n\
                       \x20 @override\n\
                       \x20 void absent() {}\n\
                       }\n";
    let names = code_names(&report_with(main_source, IFACE_LIB, true));
    assert!(
        has(&names, "override_on_non_overriding_member"),
        "absent overrides nothing on idl.X: {names:?}"
    );
}

/// A prefixed dead edge behaves like the unprefixed dead edge (`deadprefix`
/// probe): `implements idl.Missing` answers nothing, so the @override member
/// still reports — the #2420 dead-arm verdict through the prefixed probe.
#[test]
fn prefixed_dead_edge_keeps_the_override_report() {
    let main_source = "import 'lib.dart' as idl;\n\
                       \n\
                       class C implements idl.Missing {\n\
                       \x20 @override\n\
                       \x20 void absent() {}\n\
                       }\n";
    let names = code_names(&report_with(main_source, IFACE_LIB, true));
    assert!(
        has(&names, "override_on_non_overriding_member"),
        "a prefixed miss is the dead edge — nothing overrides: {names:?}"
    );
}

/// The member walk binds the prefixed face both ways: a declared member
/// resolves, a missing one is the clean miss (#2420's live-face pins through
/// the prefixed spelling).
#[test]
fn prefixed_face_member_walk_resolves_and_reports() {
    let lib_source = "class B {\n\
                      \x20 void run() {}\n\
                      }\n";
    let main_source = "import 'lib.dart' as idl;\n\
                       \n\
                       class C implements idl.B {}\n\
                       \n\
                       void f(C c) {\n\
                       \x20 c.run();\n\
                       \x20 c.absent();\n\
                       }\n";
    let names = code_names(&report_with(main_source, lib_source, true));
    assert_eq!(
        names.iter().filter(|n| **n == "undefined_method").count(),
        1,
        "run resolves through idl.B and only absent reports: {names:?}"
    );
}

/// The prefix map is keyed by the writing container: two containers
/// implementing the same local name through different prefixes each bind
/// their own spelling (a bare file-level map would cross-bind).
#[test]
fn prefixed_map_keys_by_the_writing_container() {
    let lib_source = "class A {\n\
                      \x20 void ma() {}\n\
                      }\n\
                      class B {\n\
                      \x20 void mb() {}\n\
                      }\n";
    let main_source = "import 'lib.dart' as idl;\n\
                       import 'lib.dart' as alt;\n\
                       \n\
                       class C implements idl.A {}\n\
                       class D implements alt.B {}\n\
                       \n\
                       void f(C c, D d) {\n\
                       \x20 c.ma();\n\
                       \x20 d.mb();\n\
                       }\n";
    let names = code_names(&report_with(main_source, lib_source, true));
    assert!(
        !has(&names, "undefined_method"),
        "each container binds its own prefixed face: {names:?}"
    );
}

/// A prefixed generic face binds through its argument list: `idl.Box<int>`'s
/// head is the dotted face, the `<int>` argument an argument position the
/// scan's group depth keeps out of the clause map (nested closers arrive
/// merged — `>>` is one token).
#[test]
fn prefixed_generic_face_binds_through_arguments() {
    let lib_source = "class Box<T> {\n\
                      \x20 T get value => throw '';\n\
                      }\n\
                      class Pair<A, B> {}\n";
    let main_source = "import 'lib.dart' as idl;\n\
                       \n\
                       class C implements idl.Box<int> {\n\
                       \x20 @override\n\
                       \x20 int get value => 0;\n\
                       }\n\
                       \n\
                       class D implements idl.Pair<List<int>, int> {\n\
                       \x20 void f(C c) {\n\
                       \x20   c.value;\n\
                       \x20 }\n\
                       }\n";
    let names = code_names(&report_with(main_source, lib_source, true));
    assert!(
        !has(&names, "override_on_non_overriding_getter"),
        "value is declared on the prefixed generic face: {names:?}"
    );
    assert!(
        !has(&names, "undefined_getter"),
        "the prefixed face's member resolves across the merged closer: {names:?}"
    );
}

/// The unprefixed control keeps #2420's pins green through the new dispatch:
/// a bare spelling resolves exactly as before (declared member silent,
/// missing @override reports, dead edge reports).
#[test]
fn unprefixed_control_keeps_the_2420_verdicts() {
    let good_source = "import 'lib.dart';\n\
                       \n\
                       class Good implements X {\n\
                       \x20 @override\n\
                       \x20 void hello() {}\n\
                       }\n";
    let good = code_names(&report_with(good_source, IFACE_LIB, false));
    assert!(
        !good
            .iter()
            .any(|n| n.starts_with("override_on_non_overriding")),
        "the bare import's declared member spares Good: {good:?}"
    );
    let bad_source = "import 'lib.dart';\n\
                      \n\
                      class Bad implements X {\n\
                      \x20 void hello() {}\n\
                      \x20 @override\n\
                      \x20 void absent() {}\n\
                      }\n";
    let bad = code_names(&report_with(bad_source, IFACE_LIB, false));
    assert!(
        has(&bad, "override_on_non_overriding_member"),
        "absent overrides nothing on the bare face: {bad:?}"
    );
    let dead_source = "import 'lib.dart';\n\
                       \n\
                       class Dead implements Missing {\n\
                       \x20 @override\n\
                       \x20 void absent() {}\n\
                       }\n";
    let dead = code_names(&report_with(dead_source, IFACE_LIB, false));
    assert!(
        has(&dead, "override_on_non_overriding_member"),
        "the bare dead edge answers nothing: {dead:?}"
    );
}
