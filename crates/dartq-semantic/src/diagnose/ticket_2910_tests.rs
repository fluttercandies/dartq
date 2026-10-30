//! The #2910 pins: the member-use `deprecated_member_use` /
//! `experimental_member_use` faces (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/element_usage_detector.dart +
//! deprecated_member_use_verifier.dart + experimental_member_use_verifier.dart).
//!
//! Shapes × both codes (member access, with-message, class type use /
//! instantiation) plus the suppression faces: same-package and same-library
//! uses silent, non-declared uses silent, and the frontier rule (a use
//! inside an enclosing declaration carrying the same tag never reports;
//! the OTHER tag still does — the oracle's sticky per-tag stacks).
//!
//! Behavioral oracle: dart 3.13.7 `deprecated_member_use_test.dart` /
//! `experimental_member_use_test.dart` resolution probes.

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

/// The cross-package two-library rig with a package:meta stand-in (the
/// dependency imports it; the main unit imports the dependency plain): the
/// @experimental face resolves the annotation the way the oracle does —
/// in the DECLARING library's scope.
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
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        lib,
        ImportEdge {
            target: Some(meta),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        dep_lib,
        ImportEdge {
            target: Some(meta),
            prefix: None,
            deferred: false,
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

// —— deprecated_member_use: member shapes ————————————————————————————

/// A deprecated method invoked cross-package reports once at the selector
/// with the verbatim plain message (upstream `test_methodInvocation`).
#[test]
fn dmu_method_invocation_cross_package() {
    let report = member_use_report(
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

/// A deprecated field read cross-package reports at the member token (the
/// implicit-getter shape, upstream `test_field_implicitGetter`).
#[test]
fn dmu_field_read_cross_package() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.foo; }",
        "class A { @deprecated int foo = 0; }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "'foo' is deprecated and shouldn't be used."
    );
}

/// A `@Deprecated('…')` annotation renders the normalized message tail
/// (the with-message registry shape, same printed code name — the tail
/// gains its terminal period per `normalizeDeprecationMessage`).
#[test]
fn dmu_with_message_annotation() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "class A { @Deprecated('Use bar instead') void m() {} }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "'m' is deprecated and shouldn't be used. Use bar instead."
    );
}

/// A deprecated static member read through the container fires on the
/// member row (`A.s`, the static-access shape).
#[test]
fn dmu_static_member_access() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; final x = A.s;",
        "class A { @deprecated static int s = 0; }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'s' is deprecated and shouldn't be used.");
}

// —— deprecated_member_use: container shapes ——————————————————————————

/// A deprecated class named in a type annotation reports at the name; an
/// instantiation reports once (the bare-reference and creation-head
/// publications overlap, and the dedup keeps one row).
#[test]
fn dmu_class_type_use_and_instantiation() {
    let type_use = member_use_report(
        "import 'package:s/kit.dart'; C? x;",
        "@deprecated class C {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&type_use, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{type_use:?}");
    assert_eq!(rows[0].message, "'C' is deprecated and shouldn't be used.");

    let creation = member_use_report(
        "import 'package:s/kit.dart'; var x = C();",
        "@deprecated class C {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&creation, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{creation:?}");
}

// —— experimental_member_use ————————————————————————————————————————

/// An @experimental member read cross-package reports the static-warning
/// row with the verbatim message and no correction.
#[test]
fn emu_member_read_cross_package() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.g; }",
        "import 'package:meta/meta.dart'; class A { @experimental int get g => 0; }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "experimental_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "'g' is experimental and could be removed or changed at any time."
    );
    assert_eq!(rows[0].correction, None);
}

/// An @experimental class instantiated cross-package reports once at the
/// head (upstream `ExperimentalMemberUseTest.test_annotatedClass`); a
/// plain `extends` over it reports at the clause name.
#[test]
fn emu_class_instantiation_and_extends() {
    let instantiation = member_use_report(
        "import 'package:s/kit.dart'; var x = Foo();",
        "import 'package:meta/meta.dart'; @experimental class Foo {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&instantiation, "experimental_member_use");
    assert_eq!(rows.len(), 1, "{instantiation:?}");
    assert_eq!(
        rows[0].message,
        "'Foo' is experimental and could be removed or changed at any time."
    );

    let extends_use = member_use_report(
        "import 'package:s/kit.dart'; class Bar extends Foo {}",
        "import 'package:meta/meta.dart'; @experimental class Foo {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&extends_use, "experimental_member_use");
    assert_eq!(rows.len(), 1, "{extends_use:?}");
}

/// The annotation does not propagate through the hierarchy: extending a
/// local wrapper of an experimental class stays silent (upstream
/// `test_annotatedClass_indirect`).
#[test]
fn emu_indirect_use_stays_silent() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; class Baz extends Bar {}",
        "import 'package:meta/meta.dart'; @experimental class Foo {} class Bar extends Foo {}",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "experimental_member_use").is_empty(),
        "{report:?}"
    );
}

// —— suppression faces ——————————————————————————————————————————————

/// A same-package deprecated use is silent (`isInSamePackage` — both
/// reporters bail; the `…_from_same_package` sibling code owns that face).
#[test]
fn dmu_same_package_use_is_silent() {
    let report = member_use_report(
        "import 'package:t/kit.dart'; void f(A a) { a.m(); }",
        "class A { @deprecated void m() {} }",
        "package:t/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// A use inside the declaring library itself is silent (same library is
/// trivially same-package).
#[test]
fn dmu_same_library_use_is_silent() {
    let report = member_use_report(
        "class A { @deprecated void m() {} void f(A a) { a.m(); } }",
        "class B {}",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// A non-declared member use never reports (the tag consult answers no).
#[test]
fn plain_member_use_is_silent() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "class A { void m() {} }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
    assert!(
        code_rows(&report, "experimental_member_use").is_empty(),
        "{report:?}"
    );
}

/// A cross-package deprecated MEMBER use inside an enclosing declaration
/// carrying the SAME tag is silent (the frontier's sticky deprecated
/// stack, upstream `test_inDeprecatedClass`); outside it, the same member
/// reports.
#[test]
fn dmu_inside_deprecated_declaration_is_silent() {
    let inside = member_use_report(
        "import 'package:s/kit.dart'; @deprecated class C { void m(A a) { a.m(); } }",
        "class A { @deprecated void m() {} }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&inside, "deprecated_member_use").is_empty(),
        "{inside:?}"
    );

    let outside = member_use_report(
        "import 'package:s/kit.dart'; void g(A a) { a.m(); }",
        "class A { @deprecated void m() {} }",
        "package:s/kit.dart",
    );
    assert_eq!(
        code_rows(&outside, "deprecated_member_use").len(),
        1,
        "{outside:?}"
    );
}

/// A cross-package @experimental member use inside an enclosing
/// @experimental declaration is silent (the frontier's experimental
/// stack).
#[test]
fn emu_inside_experimental_declaration_is_silent() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; @experimental class C { void m(Foo f) { f.g; } }",
        "import 'package:meta/meta.dart'; @experimental class Foo { @experimental int get g => 0; }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "experimental_member_use").is_empty(),
        "{report:?}"
    );
}

/// The shared rig's same-package gate over the plain import shape (a
/// deprecated member used from a same-package library through
/// [`report_ops_imports`], which mounts both libraries under `package:t`).
#[test]
fn dmu_same_package_through_the_shared_rig_is_silent() {
    let report = report_ops_imports(
        "import 'lib.dart'; void f(A a) { a.m(); }",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "class A { @deprecated void m() {} }",
        }],
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}
