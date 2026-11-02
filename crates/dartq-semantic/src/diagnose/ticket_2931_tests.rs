//! The #2931 pins: the constructor-USE publication — an `A.named()` /
//! `A()` creation whose resolved constructor carries `@deprecated` now
//! mints the `member_refs` edge the verifier's `member: Some` arm drains
//! (the `ctor_annotations` fallback — constructors carry no `MemberEntry`
//! row, so no member-shaped edge reached them before).
//!
//! Upstream firing condition (cited, upstream Dart SDK source):
//! `ElementUsageDetector.constructorName` reads `node.element`'s
//! metadata — pkg/analyzer/lib/src/error/element_usage_detector.dart
//! :189-191 (`checkUsage(node.element, node)`), driven from
//! `best_practices_verifier.dart` `visitConstructorName` :321-324. The
//! report anchors the `ConstructorName` node itself (the class+selector
//! compound for `A.named`, the head token for `A()`), and the display
//! name is `ConstructorElement.displayName`
//! (pkg/analyzer/lib/src/dart/element/element.dart:838-847): the
//! dot-joined `A.named` for a named face, the bare class name `A` for the
//! unnamed/`new` face.
//!
//! Behavioral oracle: dart 3.13.7 `deprecated_member_use_test.dart` —
//! `test_instanceCreation_namedConstructor` (:1144-1160),
//! `test_instanceCreation_unnamedConstructor` (:1201-1217),
//! `test_instanceCreation_undeprecatedClass_deprecatedConstructor`
//! (:1163-1179), `test_dotShorthandConstructorInvocation_…` (:529-566).

use super::*;
use crate::diagnose::test_rig::{RigImport, report_ops_imports};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

fn code_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .collect()
}

/// The same cross-package two-library rig `ticket_2910_tests` mounts: the
/// main unit imports the dependency plain; the dependency can reach
/// `package:meta` for the @experimental face.
fn member_use_report(main: &str, dependency: &str, dependency_uri: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan_fixture(main, file);
    let scanned_dep = scan_fixture(dependency, dep_file);
    let elements = crate::collector::collect(main, &scanned.tokens, features, &strings);
    let dep_elements =
        crate::collector::collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern(dependency_uri);
    let meta = builder.intern("package:meta/meta.dart");
    builder.library_name(meta, "meta");
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
    builder.import(
        lib,
        ImportEdge {
            target: Some(meta),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        dep_lib,
        ImportEdge {
            target: Some(meta),
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

// —— deprecated_member_use: the unnamed-constructor use ————————————————

/// A deprecated unnamed constructor invoked cross-package (`A()`) reports
/// at the head token — upstream `ConstructorElement.displayName` reads
/// the class name for the unnamed face (element.dart:845 `else
/// className`), so the message carries `'A'`.
#[test]
fn dmu_unnamed_constructor_use() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { A(); }",
        "class A { @deprecated A(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'A' is deprecated and shouldn't be used.");
}

/// The `new A()` spelling resolves the same unnamed face — one report at
/// the `A` head (upstream `test_instanceCreation_unnamedConstructor`,
/// :1213-1215: `^` under `A`).
#[test]
fn dmu_new_unnamed_constructor_use() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { new A(); }",
        "class A { @deprecated A(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'A' is deprecated and shouldn't be used.");
}

/// The `.new` selector names the unnamed face the same way — `A.new()`
/// keys `ctor_annotations` under the class spelling and reports `'A'`.
#[test]
fn dmu_new_selector_constructor_use() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { A.new(); }",
        "class A { @deprecated A(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'A' is deprecated and shouldn't be used.");
}

// —— deprecated_member_use: the named-constructor use ——————————————————

/// A deprecated named constructor invoked cross-package reports once.
/// Upstream's displayName is the dot-joined `'A.named'`
/// (element.dart:843); the verifier's member arm displays the edge's
/// `target_name` verbatim, so this model emits the selector spelling —
/// the code fires at the `A.named` compound with the selector's own
/// display. Residual: the `A.` display prefix is a known-diffs entry.
#[test]
fn dmu_named_constructor_use() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { A.named(); }",
        "class A { @deprecated A.named(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "'named' is deprecated and shouldn't be used."
    );
}

/// The `new A.named()` spelling lands the same edge — the reduced
/// parser's constructor-reference posture folds through the named-
/// selector arm.
#[test]
fn dmu_new_named_constructor_use() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { new A.named(); }",
        "class A { @deprecated A.named(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "'named' is deprecated and shouldn't be used."
    );
}

// —— tag-absent controls —————————————————————————————————————————————

/// An undeprecated constructor under an undeprecated class stays silent.
#[test]
fn dmu_plain_constructor_is_silent() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { A.named(); A(); }",
        "class A { A.named(); A(); }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// A deprecated class with an undeprecated constructor reports the CLASS
/// star through the type-use face — the ctor edge consults
/// `ctor_annotations` and finds none, so the single row carries `'A'`
/// (upstream `test_instanceCreation_deprecatedClass_
/// undeprecatedConstructor`, :1106-1122).
#[test]
fn dmu_deprecated_class_plain_constructor() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { A(); }",
        "@deprecated class A { A(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'A' is deprecated and shouldn't be used.");
}

/// A deprecated class AND a deprecated constructor report ONCE — the
/// type-use and member-edge faces answer the same declaration at the
/// same offset and the verifier's `(offset, tag)` set dedups (upstream
/// `test_instanceCreation_deprecatedClass_deprecatedConstructor`,
/// :1086-1103).
#[test]
fn dmu_deprecated_class_and_constructor_report_once() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { A(); }",
        "@deprecated class A { @deprecated A(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'A' is deprecated and shouldn't be used.");
}

/// A member call ON a created instance (`A().named()`) never consults the
/// constructor face — the `Receiver::Created` arm stays out of the edge.
#[test]
fn dmu_member_on_created_instance_is_silent() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f() { A().named(); }",
        "class A { @deprecated A.named(); A(); }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

// —— suppression faces ———————————————————————————————————————————————

/// The `isInSamePackage` gate: a same-package deprecated constructor use
/// never reports (upstream reporters bail before reading the tag).
#[test]
fn dmu_constructor_use_same_package_is_silent() {
    let report = report_ops_imports(
        "import 'lib.dart'; void f() { A.named(); A(); }",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class A { @deprecated A.named(); @deprecated A(); }",
        }],
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}
