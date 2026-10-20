//! The #2329 differential pins: the annotation name's resolution order.
//! Upstream resolves the annotation expression like any constant-context
//! expression inside the metadata scope (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/summary2/metadata_resolver.dart` — member metadata
//! in the container's body scope, head metadata before it opens;
//! `pkg/analyzer/lib/src/dart/element/scope.dart` — `InstanceScope` holds
//! only the container's own members, `ExtensionScope` every own extension
//! member, neither climbs the supertype chain), so the climbed faces never
//! answer: a superclass `static const internal` and an own *instance*
//! member stay missing into `undefined_annotation` (probe-verified against
//! the 3.13.1 oracle), while locals, container type parameters, own
//! statics and top-level names resolve silently.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The single-file rig: every declaration indexed, `dart:core` seeded with
/// the unindexed SDK face (the engine driver's posture — the annotations
/// under test name nothing from it).
fn report(main_source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        main_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {main_source:?}"
    );
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &["Object", "String", "int", "bool", "double", "num", "Match"],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
        main_source,
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
    )
}

fn codes(report: &FileReport, name: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The ticket's face: `@internal` over a subclass member while the
/// *superclass* declares `static const internal`. The supertype climb is
/// not part of the annotation scope order (probe: the oracle reports
/// `Undefined name 'internal' used as an annotation.`), so the name misses
/// and `undefined_annotation` anchors over the `@internal` span.
#[test]
fn superclass_static_const_does_not_answer_annotation() {
    let source = "class A {\n\
                  \x20 static const internal = 1;\n\
                  }\n\
                  \n\
                  class B extends A {\n\
                  \x20 @internal\n\
                  \x20 void m() {}\n\
                  }\n";
    let report = report(source);
    let at = source.find("@internal").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![(at, "@internal".len() as u32)],
        "{:?}",
        report.diagnostics
    );
}

/// The own-instance twin (probe `g_own_instance.dart`): an own *instance*
/// member is equally outside the annotation scope order — `@internal` over
/// a class body whose class declares `int internal` still reports, where
/// the member-face resolution used to claim it silent.
#[test]
fn own_instance_member_does_not_answer_annotation() {
    let source = "class A {\n\
                  \x20 int internal = 0;\n\
                  \n\
                  \x20 @internal\n\
                  \x20 void m() {}\n\
                  }\n";
    let report = report(source);
    let at = source.find("@internal").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![(at, "@internal".len() as u32)],
        "{:?}",
        report.diagnostics
    );
}

/// The guard twin (probe `e_own_static.dart`): an own *static* const IS in
/// the annotation scope (`InstanceScope` carries the own statics), so
/// `@internal` over the declaring class's own `static const internal`
/// stays silent — the scope order must not over-report here.
#[test]
fn own_static_const_answers_annotation() {
    let source = "class A {\n\
                  \x20 static const internal = 1;\n\
                  \n\
                  \x20 @internal\n\
                  \x20 void m() {}\n\
                  }\n";
    let report = report(source);
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![],
        "{:?}",
        report.diagnostics
    );
    assert!(
        codes(&report, "undefined_identifier").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// A top-level constant resolves through the scoped lookup and stays
/// silent (probe `b_top_level.dart`) — the order's last layer answers
/// before the miss.
#[test]
fn top_level_const_annotation_resolves() {
    let source = "const topLevelConst = 1;\n\
                  \n\
                  class C {\n\
                  \x20 @topLevelConst\n\
                  \x20 void m() {}\n\
                  }\n";
    let report = report(source);
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![],
        "{:?}",
        report.diagnostics
    );
    assert!(
        codes(&report, "undefined_identifier").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// A local constant shadows the annotation name: metadata on a local
/// function declaration resolves with the enclosing frame's locals in
/// scope (probe `c_local_shadow.dart` — the oracle stays silent), so the
/// local wins before any scope lookup.
#[test]
fn local_const_shadows_annotation_name() {
    let source = "void f() {\n\
                  \x20 const localShadow = 1;\n\
                  \n\
                  \x20 @localShadow\n\
                  \x20 void g() {}\n\
                  }\n";
    let report = report(source);
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![],
        "{:?}",
        report.diagnostics
    );
}

/// The sensitivity twin of the local-shadow pin: without the local, the
/// same local-function annotation misses every layer and reports — proof
/// the local-function metadata path is exercised at all (the shadow pin's
/// silence is the resolution order, not a dead path).
#[test]
fn local_function_annotation_miss_reports() {
    let source = "void f() {\n\
                  \x20 @noSuchLocal\n\
                  \x20 void g() {}\n\
                  }\n";
    let report = report(source);
    let at = source.find("@noSuchLocal").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![(at, "@noSuchLocal".len() as u32)],
        "{:?}",
        report.diagnostics
    );
}

/// The container's own type parameters are in the annotation scope
/// (probe `d_type_param.dart` — the oracle stays silent); the method's own
/// type parameters are not (metadata precedes the header), pinned by the
/// twin below.
#[test]
fn class_type_param_annotation_resolves() {
    let source = "class C<T> {\n\
                  \x20 @T\n\
                  \x20 void m() {}\n\
                  }\n";
    let report = report(source);
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![],
        "{:?}",
        report.diagnostics
    );
}

/// The method's own type parameter is not yet in scope at its metadata
/// (probe `l_method_type_param.dart` — the oracle reports), so `@T` over
/// `void m<T>()` misses into `undefined_annotation`.
#[test]
fn method_type_param_does_not_answer_annotation() {
    let source = "class C {\n\
                  \x20 @T\n\
                  \x20 void m<T>() {}\n\
                  }\n";
    let report = report(source);
    let at = source.find("@T").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![(at, "@T".len() as u32)],
        "{:?}",
        report.diagnostics
    );
}

/// An extension body's own members (static and instance alike) are in the
/// annotation scope (`ExtensionScope` carries every own member; probe
/// `i_extension_body_static.dart` / `j_extension_body_instance.dart` —
/// the oracle stays silent both ways).
#[test]
fn extension_body_own_members_answer_annotation() {
    let source = "extension E on int {\n\
                  \x20 static const internal = 1;\n\
                  \n\
                  \x20 @internal\n\
                  \x20 void m() => 0;\n\
                  }\n";
    let report = report(source);
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![],
        "{:?}",
        report.diagnostics
    );
}
