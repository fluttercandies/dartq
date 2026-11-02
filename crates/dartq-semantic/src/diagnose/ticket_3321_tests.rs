//! The #3321 pins: `deprecated_member_use` on the ENGINE graph posture.
//!
//! The #2910/#2939 rigs model a graph with no `dart:core` node, so a bare
//! `@deprecated` spelling missed the import namespace and the tag test's one
//! arm answered it. The engine registers `dart:core` as a real node
//! (generated name face or loaded units) and wires the implicit import edge
//! into every live library, so the same spelling RESOLVES into dart:core —
//! the arm the tag test did not have, which left the whole face silent on
//! every cross-package use. These pins seed the implicit core face
//! (`LibraryGraphBuilder::seed_implicit_core`) and therefore reproduce the
//! engine's resolution; the unit-level pair for the tag test itself lives in
//! `crate::annotation`.
//!
//! Behavioral oracle: dart 3.13.7 `deprecated_member_use_test.dart`
//! (`DeprecatedMemberUse_PackageConfigWorkspaceTest`).

use super::*;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The `dart:core` names the tag arm resolves through — the two the
/// annotation face tests plus a non-tag control.
const CORE_FACE: &[&str] = &["deprecated", "Deprecated", "Object"];

fn code_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .collect()
}

/// The cross-library rig with the engine's implicit `dart:core` edge on both
/// libraries: the declaring unit is `dependency` (uri `dependency_uri`), the
/// analyzed unit `main` (uri `package:app/main.dart`) imports it plain.
fn core_member_use_report(main: &str, dependency: &str, dependency_uri: &str) -> FileReport {
    core_member_use_report_prefixed(main, dependency, dependency_uri, None)
}

/// [`core_member_use_report`] with an optional `import 'dart:core' as
/// <prefix>;` written in the DECLARING library — the prefixed tag face.
fn core_member_use_report_prefixed(
    main: &str,
    dependency: &str,
    dependency_uri: &str,
    dep_core_prefix: Option<&'static str>,
) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan_fixture(main, file);
    let scanned_dep = scan_fixture(dependency, dep_file);
    let elements = crate::collector::collect(main, &scanned.tokens, features, &strings);
    let dep_elements =
        crate::collector::collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:app/main.dart");
    let dep_lib = builder.intern(dependency_uri);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in dep_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(dep_lib, name, dep_file);
    }
    builder.unit(lib, file);
    builder.unit(dep_lib, dep_file);
    // The engine merges the implicit core edge in front of the written ones,
    // and registers the core node's names-only face file as a unit — the
    // `file -> library` row the tag arm's dart:core identity test reads.
    let core = builder.intern("dart:core");
    builder.face_unit(core, core_file);
    builder.seed_implicit_core(lib, core_file, CORE_FACE);
    builder.seed_implicit_core(dep_lib, core_file, CORE_FACE);
    if let Some(prefix) = dep_core_prefix {
        let prefix_id = strings.intern(prefix);
        builder.import(
            dep_lib,
            ImportEdge {
                target: Some(core),
                prefix: Some(prefix_id),
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(dep_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    fold_unit_with_ops(
        main,
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
        false,
        None,
    )
    .report
}

fn scan_fixture(source: &str, file: dartq_source::FileId) -> dartq_scanner::ScanResult {
    let scanned = dartq_scanner::scan(
        source,
        dartq_scanner::ScanConfig {
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
}

// —— the dominant cluster: a bare tag resolving into dart:core ————————

/// The regression: a deprecated method invoked cross-package reports once at
/// the selector. Before the fix the tag arm only accepted a namespace MISS,
/// and the engine's implicit core edge turns this spelling into a HIT in
/// dart:core — zero rows for every shape in the family.
#[test]
fn dmu_engine_graph_method_invocation() {
    let report = core_member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "class A { @deprecated void m() {} }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'m' is deprecated and shouldn't be used.");
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try replacing the use of the deprecated member with the replacement.")
    );
}

/// A deprecated top-level variable read cross-package reports at the
/// reference (the `test_topLevelVariable_*` family's base shape).
#[test]
fn dmu_engine_graph_top_level_variable() {
    let report = core_member_use_report(
        "import 'package:s/kit.dart'; void g() { var y = x; }",
        "@deprecated int x = 0;",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'x' is deprecated and shouldn't be used.");
}

/// A deprecated class named in a type annotation reports at the name (the
/// `Deprecated` class tag through the same implicit edge).
#[test]
fn dmu_engine_graph_class_type_use() {
    let report = core_member_use_report(
        "import 'package:s/kit.dart'; C? y;",
        "@deprecated class C {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'C' is deprecated and shouldn't be used.");
}

/// The prefixed face: `@core.deprecated` is the tag under an
/// `import 'dart:core' as core;` written in the DECLARING library.
#[test]
fn dmu_engine_graph_prefixed_core_tag() {
    let report = core_member_use_report_prefixed(
        "import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "import 'dart:core' as core; class A { @core.deprecated void m() {} }",
        "package:s/kit.dart",
        Some("core"),
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'m' is deprecated and shouldn't be used.");
}

// —— the silent negatives the fix must not break ————————————————

/// Same package: both reporters bail before reading the tag
/// (`isInSamePackage`, deprecated_member_use_verifier.dart:46).
#[test]
fn dmu_engine_graph_same_package_silent() {
    let report = core_member_use_report(
        "import 'package:app/kit.dart'; void f(A a) { a.m(); }",
        "class A { @deprecated void m() {} }",
        "package:app/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// The frontier rule: a use inside the analyzed unit's own `@deprecated`
/// declaration never reports (`ElementUsageFrontierDetector`'s sticky
/// stack). Now that the tag arm fires, this suppression has to hold.
#[test]
fn dmu_engine_graph_frontier_suppresses_inner_use() {
    let report = core_member_use_report(
        "import 'package:s/kit.dart'; class B { @deprecated void host() { var y = x; } }",
        "@deprecated int x = 0;",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// An untagged declaration stays silent — the fix must not fire the face
/// wholesale.
#[test]
fn dmu_engine_graph_untagged_silent() {
    let report = core_member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "class A { void m() {} }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// A same-spelled non-core annotation is not the tag: the declaring library
/// resolves `deprecated` to its own class, never to dart:core.
#[test]
fn dmu_engine_graph_shadowed_tag_is_not_the_core_tag() {
    let report = core_member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "class deprecated { const deprecated(); } class A { @deprecated void m() {} }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}
