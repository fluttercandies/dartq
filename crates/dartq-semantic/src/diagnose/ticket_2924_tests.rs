//! The #2924 pins: the declaration faces the #2910 member-use reporter
//! could not see — top-level functions/variables/typedefs, named formal
//! parameters (the call-site `name:` label anchor, upstream
//! `element_usage_detector.dart` `_visitParametersAndArguments`, :381-413),
//! named-extension uses (`E(0).foo`, `visitExtensionOverride`), named
//! deprecated constructors (`A.named()`, `A.new`), the prefixed
//! `@core.deprecated` spelling, and the `@deprecated library` whole-file
//! frontier (`pushElement(_currentLibrary)` — best_practices_verifier.dart
//! :166).
//!
//! Behavioral oracle: dart 3.13.7 `deprecated_member_use_test.dart`
//! resolution probes for the top-level / parameter / extension /
//! constructor faces.

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
/// main unit imports the dependency plain; both libraries can reach
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
    // The `@core.deprecated` prefixed face: the declaring library reaches
    // dart:core through a `core` prefix edge (the rig otherwise relies on
    // the implicit import, which the prefixed arm cannot resolve through).
    let core = builder.intern("dart:core");
    builder.library_name(core, "dart.core");
    builder.import(
        dep_lib,
        ImportEdge {
            target: Some(core),
            prefix: Some(strings.intern("core")),
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

// —— deprecated_member_use: top-level declaration faces ————————————————

/// A deprecated top-level function invoked cross-package reports at the
/// call's name token (upstream `test_topLevelFunction`).
#[test]
fn dmu_top_level_function_call() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { f(); }",
        "@deprecated void f() {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'f' is deprecated and shouldn't be used.");
}

/// A deprecated top-level variable read cross-package reports at the name
/// (the getter shape — upstream `test_topLevelVariable`).
#[test]
fn dmu_top_level_variable_read() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; final x = v;",
        "@deprecated final v = 0;",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'v' is deprecated and shouldn't be used.");
}

/// A deprecated typedef named in a type annotation reports at the name
/// (upstream `test_typedef` / the type-use publication).
#[test]
fn dmu_typedef_type_use() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; T? x;",
        "@deprecated typedef T = int;",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'T' is deprecated and shouldn't be used.");
}

// —— deprecated_member_use: the named-parameter face ———————————————————

/// A `name:` argument bound to a deprecated named formal reports at the
/// LABEL — the NamedExpression's own anchor, not the callee's (upstream
/// `test_namedParameter`, `_visitParametersAndArguments`).
#[test]
fn dmu_deprecated_named_parameter_top_level() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { f(p: 1); }",
        "void f({@deprecated int? p}) {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'p' is deprecated and shouldn't be used.");
}

/// The same face through a member callee (`a.m(p: 1)`), the declaring
/// owner resolved off the member walk's `found_origin`.
#[test]
fn dmu_deprecated_named_parameter_member() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g(A a) { a.m(p: 1); }",
        "class A { void m({@deprecated int? p}) {} }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'p' is deprecated and shouldn't be used.");
}

/// A label bound to an untagged formal stays silent (the tag-absent
/// direction of the same consult).
#[test]
fn dmu_plain_named_parameter_is_silent() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { f(p: 1); }",
        "void f({int? p}) {}",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

// —— deprecated_member_use: the named-extension face ———————————————————

/// A named extension's application (`E(0).foo`) reports the extension's
/// own tag at the head (upstream `visitExtensionOverride` reads the
/// extension element's metadata — a use of `E` is a use of the extension).
#[test]
fn dmu_named_extension_application() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; final x = E(0).foo;",
        "@deprecated extension E on int { int get foo => 1; }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'E' is deprecated and shouldn't be used.");
}

/// A deprecated member of a named extension reports on the member's
/// selector (the member arm over the extension face).
#[test]
fn dmu_extension_member_use() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; final x = E(0).foo;",
        "extension E on int { @deprecated int get foo => 1; }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "'foo' is deprecated and shouldn't be used."
    );
}

// —— deprecated_member_use: the named-constructor face —————————————————

/// The named constructor's star joins the `ctor_annotations` side-map
/// under the `ctor_formals` lookup spelling — the model face the
/// deprecated named-constructor arm reads (`A.named()`'s selector keys
/// `ctor_annotations[A][named]`). Pin the capture: the star lands on the
/// named entry beside — not inside — the class star.
#[test]
fn dmu_named_constructor_star_captured() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let source = "class A { @deprecated A.named(); }";
    let scanned = scan_fixture(source, file);
    let elements = crate::collector::collect(source, &scanned.tokens, features, &strings);
    let a = strings.intern("A");
    let named = strings.intern("named");
    let star = elements
        .ctor_annotations(a, named)
        .expect("named ctor star");
    assert_eq!(star.len(), 1, "{star:?}");
    assert_eq!(strings.resolve(star[0].name), "deprecated");
    // The class star stays clean — the ctor star is a sibling, never a
    // merge into `Container::annotations`.
    assert!(
        elements
            .container(a)
            .expect("container")
            .annotations
            .is_empty(),
        "{elements:?}"
    );
}

/// The unnamed constructor's star keys under the class's own spelling
/// (`A()`'s `ctor_formals` key is `A` — the bare head normalizes the same
/// way `new` does).
#[test]
fn dmu_unnamed_constructor_star_captured() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let source = "class A { @deprecated A(); }";
    let scanned = scan_fixture(source, file);
    let elements = crate::collector::collect(source, &scanned.tokens, features, &strings);
    let a = strings.intern("A");
    let star = elements.ctor_annotations(a, a).expect("unnamed ctor star");
    assert_eq!(star.len(), 1, "{star:?}");
    assert_eq!(strings.resolve(star[0].name), "deprecated");
}

/// An untagged constructor carries no `ctor_annotations` row (the
/// tag-absent direction — the verifier's consult answers `None`).
#[test]
fn dmu_plain_constructor_star_absent() {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let source = "class A { A.named(); }";
    let scanned = scan_fixture(source, file);
    let elements = crate::collector::collect(source, &scanned.tokens, features, &strings);
    let a = strings.intern("A");
    assert!(
        elements.ctor_annotations(a, strings.intern("named")).is_none(),
        "{elements:?}"
    );
}

// —— recognition gaps ————————————————————————————————————————————————

/// The prefixed spelling `@core.deprecated` resolves the dart:core tag
/// through the import's prefix edge (`import 'dart:core' as core`) the
/// same as the bare `deprecated` getter — a member annotated with the
/// prefixed form still reports its uses.
#[test]
fn dmu_prefixed_core_deprecated_annotation() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "import 'dart:core' as core; class A { @core.deprecated void m() {} }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'m' is deprecated and shouldn't be used.");
}

/// `@deprecated library lib;` silences the tag across the whole analyzing
/// unit — the frontier detector pushes the LIBRARY element's tag onto the
/// same sticky stack as every declaration's
/// (`pushElement(_currentLibrary)`), so a cross-package member use inside
/// a deprecated library never reports.
#[test]
fn dmu_deprecated_library_directive_frontier() {
    let report = member_use_report(
        "@deprecated library m; import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "class A { @deprecated void m() {} }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// The frontier is a suppressor, not a use — a deprecated library
/// directive over an untagged member use stays silent for the right
/// reason (the tag consult itself answers no), and the library face adds
/// no report of its own.
#[test]
fn dmu_deprecated_library_directive_over_plain_use() {
    let report = member_use_report(
        "@deprecated library m; import 'package:s/kit.dart'; void f(A a) { a.m(); }",
        "class A { void m() {} }",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

// —— suppression faces ————————————————————————————————————————————————

/// The shared rig's same-package gate over a top-level function use (the
/// `…_from_same_package` sibling code owns that face).
#[test]
fn dmu_top_level_same_package_is_silent() {
    let report = report_ops_imports(
        "import 'lib.dart'; void g() { f(); }",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: None,
            source: "@deprecated void f() {}",
        }],
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}
