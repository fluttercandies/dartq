//! The #3513 pins: `deprecated_member_use` across the package boundary —
//! the member-use face over a DEPENDENCY unit, in the exact shapes the
//! conformance corpus exercises.
//!
//! The ticket's premise, corrected on the record: the member-use face has
//! existed since #2910 (`best_practices_verifier.rs`
//! `report_member_use_warnings`, driven from `finalize_variance`) and fires
//! cross-package; the #3501 recon's "10 missing census rows
//! (`hasUnsoundNullSafety` in `features/_core/function_subtype`)" were
//! measured with the 2026-11-01 09:05 stale binary, which predates #3321
//! (8e25892a, 10:26) — the commit that taught the dart:core tag test the
//! engine's hit arm, after which the member face went live on real runs
//! (`conformance/status.tsv`: `deprecated_member_use` 63 → 124 passing
//! cases; the 22:09 fresh-HEAD census passes every `function_subtype` case
//! that carried one of those rows). These pins lock the corpus shapes so
//! the face cannot go silent again, beside the `dart:` directive face's own
//! pins (`ticket_2326_tests`). Adding a second member-use pass next to the
//! directive face in `unused_finalization.rs` would double-report every use
//! and is deliberately not done.
//!
//! Behavioral oracle: dart 3.13.7 — the `function_subtype` corpus cases
//! (`cast2_test`'s bare getter read as a call argument), the
//! `DeprecatedMemberUse_PackageConfigWorkspaceTest` family, and
//! `deprecated_member_use_verifier.dart`'s `isInSamePackage` bail.

use super::*;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The `dart:core` names the corpus shapes resolve through — the tag pair,
/// the types the fixture heads spell, and the two calls the mains make.
const CORE_FACE: &[&str] = &[
    "deprecated",
    "Deprecated",
    "Object",
    "bool",
    "int",
    "String",
    "Null",
    "List",
    "print",
];

fn code_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .collect()
}

/// The two-package rig under the engine graph posture: the declaring unit
/// is `dependency` (uri `dependency_uri`), the analyzed unit `main` (uri
/// `main_uri` — a `package:` spelling or a bare `file:` one, the corpus's
/// non-package test layout) imports it plain, and BOTH libraries carry the
/// implicit `dart:core` edge into a real `dart:core` node — the arm the
/// tag test answers on every real run.
fn dep_member_use_report(
    main: &str,
    main_uri: &str,
    dependency: &str,
    dependency_uri: &str,
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
    let lib = builder.intern(main_uri);
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
    // The engine's shape: the core node's names-only face file is a unit
    // (the `file -> library` row the tag arm's dart:core identity test
    // reads) and the implicit edge rides every library.
    let core = builder.intern("dart:core");
    builder.face_unit(core, core_file);
    builder.seed_implicit_core(lib, core_file, CORE_FACE);
    builder.seed_implicit_core(dep_lib, core_file, CORE_FACE);
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

/// The corpus regression: a deprecated top-level GETTER of a dependency
/// library read as a bare call argument inside `main` reports once at the
/// identifier, with the normalized annotation tail. This is the exact
/// `features/_core/function_subtype/cast2_test` shape — a non-package
/// (`file:` uri) analyzed unit, `package:expect` declaring the getter under
/// a `library expect;` directive, and the read inside a creation-receiver
/// send (`new Class<Null>().test(hasUnsoundNullSafety, …)`).
#[test]
fn dmu_dep_library_getter_read_as_call_argument() {
    let dep = "library expect;\n\
               \n\
               @Deprecated('Use unsoundNullSafety from variations.dart instead')\n\
               bool get hasUnsoundNullSafety => const <Null>[] is List<Object>;\n";
    let main = "import 'package:expect/expect.dart';\n\
                class Class<T> {\n\
                \x20 test(bool expectedResult, o, String typeName) {}\n\
                }\n\
                void bar(int i) {}\n\
                void main() {\n\
                \x20 new Class<Null>().test(hasUnsoundNullSafety, bar, 'Null');\n\
                }\n";
    let report = dep_member_use_report(
        main,
        "file:///w/language/function_subtype/cast2_test.dart",
        dep,
        "package:expect/expect.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message.as_str(),
        "'hasUnsoundNullSafety' is deprecated and shouldn't be used. \
         Use unsoundNullSafety from variations.dart instead."
    );
    let at = main.find("hasUnsoundNullSafety").unwrap() as u32;
    assert_eq!(rows[0].range.offset, at, "{report:?}");
    assert_eq!(rows[0].range.length, 20, "{report:?}");
    assert_eq!(rows[0].severity, dartq_diagnostics::Severity::Info);
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try replacing the use of the deprecated member with the replacement.")
    );
}

/// The two-package path-dependency shape: `pkg_b` reads `pkg_a`'s deprecated
/// top-level VARIABLE in an `if` condition — the message tail renders from
/// the annotation's argument text, gaining the terminal period.
#[test]
fn dmu_path_dependency_variable_read_in_condition() {
    let dep = "@Deprecated('use currentFlag')\n\
               bool hasUnsoundNullSafety = false;\n";
    let main = "import 'package:pkg_a/a.dart';\n\
                void probe() {\n\
                \x20 if (hasUnsoundNullSafety) {\n\
                \x20   print('legacy');\n\
                \x20 }\n\
                }\n";
    let report = dep_member_use_report(main, "package:pkg_b/b.dart", dep, "package:pkg_a/a.dart");
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message.as_str(),
        "'hasUnsoundNullSafety' is deprecated and shouldn't be used. use currentFlag."
    );
    let at = main.find("hasUnsoundNullSafety").unwrap() as u32;
    assert_eq!((rows[0].range.offset, rows[0].range.length), (at, 20));
}

/// The silent control: the SAME-PACKAGE read never reports under this code
/// — `isInSamePackage` bails the reporter (deprecated_member_use_verifier
/// .dart:46) and the `deprecated_member_use_from_same_package` variant owns
/// the report, which is a dartq-lints face and deliberately not implemented
/// here.
#[test]
fn dmu_same_package_use_stays_silent() {
    let dep = "@Deprecated('use currentFlag')\n\
               bool hasUnsoundNullSafety = false;\n";
    let main = "import 'package:pkg_a/a.dart';\n\
                void probe() {\n\
                \x20 if (hasUnsoundNullSafety) {}\n\
                }\n";
    let report = dep_member_use_report(main, "package:pkg_a/b.dart", dep, "package:pkg_a/a.dart");
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// The plain shape: a dependency getter tagged with the bare `@deprecated`
/// getter (upstream reads its `PropertyAccessorElement` annotation as
/// no-message) reports without the message tail.
#[test]
fn dmu_plain_tag_getter_reports_the_plain_shape() {
    let dep = "@deprecated\n\
               bool get oldFlag => false;\n";
    let main = "import 'package:s/kit.dart';\n\
                void probe() {\n\
                \x20 while (oldFlag) {}\n\
                }\n";
    let report = dep_member_use_report(main, "package:app/main.dart", dep, "package:s/kit.dart");
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message.as_str(),
        "'oldFlag' is deprecated and shouldn't be used."
    );
}

/// Both faces of the code co-exist in one unit: the deprecated `dart:` set
/// reports at its whole import directive while the dependency getter's use
/// reports at the identifier — no face swallows the other.
#[test]
fn dmu_directive_and_member_faces_report_independently() {
    let dep = "@deprecated\n\
               bool get oldFlag => false;\n";
    let main = "import 'dart:html';\n\
                import 'package:s/kit.dart';\n\
                void probe() {\n\
                \x20 if (oldFlag) {}\n\
                }\n";
    let report = dep_member_use_report(main, "package:app/main.dart", dep, "package:s/kit.dart");
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 2, "{report:?}");
    let directive = rows
        .iter()
        .find(|row| row.range.offset == 0)
        .expect("directive row");
    assert_eq!(directive.range.length, 19);
    assert_eq!(
        directive.message.as_str(),
        "'dart:html' is deprecated and shouldn't be used. \
         Use package:web and dart:js_interop instead."
    );
    let member = rows
        .iter()
        .find(|row| row.range.offset != 0)
        .expect("member row");
    assert_eq!(
        member.message.as_str(),
        "'oldFlag' is deprecated and shouldn't be used."
    );
}
