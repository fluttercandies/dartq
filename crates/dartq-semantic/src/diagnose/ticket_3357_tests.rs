//! The #3357 pins: the access-annotation family
//! (`invalid_use_of_protected_member`,
//! `invalid_use_of_visible_for_testing_member`,
//! `invalid_use_of_visible_for_overriding_member` — upstream Dart SDK
//! source, pkg/analyzer/lib/src/error/best_practices_verifier.dart
//! `_checkForOtherInvalidAccess` :1824-1905) and the
//! `type_annotation_deferred_class` arm (`error_verifier.dart`
//! `_checkForTypeAnnotationDeferredClass` :7132-7139).
//!
//! The positive pins mirror the oracle's own fixtures (`InvalidUseOf…Test`,
//! `TypeAnnotationDeferredClassTest`, the `doc_*` examples): the message
//! text, the anchor span, and the silent negatives — a subclass access, a
//! same-library access, the override's own `super.x`, a supertype clause,
//! a function type's return type, and a non-deferred prefix.

use super::*;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_diagnostics::Severity;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The package:meta stand-ins the access annotations resolve through
/// (`library meta;` — the library name `_isPackageMetaGetter` reads,
/// element.dart:1661/:2010-2012).
const META_MOCK: &str = r#"library meta;

const protected = _Protected();
const visibleForTesting = _VisibleForTesting();
const visibleForOverriding = _VisibleForOverriding();

class _Protected { const _Protected(); }
class _VisibleForTesting { const _VisibleForTesting(); }
class _VisibleForOverriding { const _VisibleForOverriding(); }
"#;

fn code_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .collect()
}

/// The offset of `needle` in `main` plus `inner` (the anchor's own column
/// inside the matched text).
fn at(main: &str, needle: &str, inner: usize) -> SourceRange {
    let base = main.find(needle).unwrap_or_else(|| panic!("no {needle:?}"));
    SourceRange {
        offset: (base + inner) as u32,
        length: 1,
    }
}

/// The offset of the LAST `needle` in `main` plus `inner`.
fn at_last(main: &str, needle: &str, inner: usize) -> SourceRange {
    let base = main
        .rfind(needle)
        .unwrap_or_else(|| panic!("no {needle:?}"));
    SourceRange {
        offset: (base + inner) as u32,
        length: 1,
    }
}

/// The shared rig: `main` (unit `package:t/main.dart`) imports the library
/// under test through `prefix` (deferred when `deferred`), and both import
/// the package:meta mock the annotation predicates resolve through.
fn rig(main: &str, lib1: &str, prefix: Option<&str>, deferred: bool) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let lib1_file = files.intern(Path::new("lib1.dart"));
    let meta_file = files.intern(Path::new("meta.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan_fixture(main, file);
    let elements = crate::collector::collect(main, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let lib1_lib = builder.intern("package:t/lib1.dart");
    let meta_lib = builder.intern("package:meta/meta.dart");
    builder.library_name(meta_lib, "meta");
    let meta_elements = crate::collector::collect(
        META_MOCK,
        &scan_fixture(META_MOCK, meta_file).tokens,
        features,
        &strings,
    );
    let lib1_elements = crate::collector::collect(
        lib1,
        &scan_fixture(lib1, lib1_file).tokens,
        features,
        &strings,
    );
    for (unit, unit_file, unit_elements) in [
        (lib, file, &elements),
        (lib1_lib, lib1_file, &lib1_elements),
        (meta_lib, meta_file, &meta_elements),
    ] {
        for (name, decl) in unit_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(unit, name, unit_file);
        }
        builder.unit(unit, unit_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(lib1_lib),
            prefix: prefix.map(|name| strings.intern(name)),
            non_library: false,
            deferred,
            combinators: Combinators::unfiltered(),
        },
    );
    for from in [lib, lib1_lib] {
        builder.import(
            from,
            ImportEdge {
                target: Some(meta_lib),
                prefix: None,
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(lib1_file, lib1_elements);
    index.insert(meta_file, meta_elements);
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

// —— invalid_use_of_protected_member ————————————————————————————————

/// A `@protected` member read from another library outside any subclass
/// reports at the selector with the declaring class's display name
/// (upstream `InvalidUseOfProtectedMemberTest/doc_0`).
#[test]
fn protected_member_outside_subclass_reports() {
    let lib1 = "import 'package:meta/meta.dart';\n\nclass A {\n  @protected\n  void a() {}\n}\n";
    let main = "import 'lib1.dart';\n\nvoid b(A a) {\n  a.a();\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_protected_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Warning);
    assert_eq!(
        rows[0].message,
        "The member 'a' can only be used within instance members of subclasses of 'A'."
    );
    assert_eq!(rows[0].range, at(main, "a.a()", 2));
}

/// The sanctioned use: an access inside a subclass of the declaring class
/// is exempt (upstream `test_extendingSubclass`, `test_getter_subclass`).
#[test]
fn protected_member_in_subclass_silent() {
    let lib1 =
        "import 'package:meta/meta.dart';\n\nclass A {\n  @protected\n  int get a => 0;\n}\n";
    let main = "import 'lib1.dart';\n\nclass B extends A {\n  int b() => a;\n}\n";
    let report = rig(main, lib1, None, false);
    assert!(
        code_rows(&report, "invalid_use_of_protected_member").is_empty(),
        "{report:?}"
    );
}

/// A same-library access is exempt before the annotation consult
/// (`_inCurrentLibrary`, upstream `test_sameLibrary`).
#[test]
fn protected_member_same_library_silent() {
    let lib1 = "";
    let main = "import 'package:meta/meta.dart';\n\nclass A {\n  @protected\n  void a() {}\n}\n\nclass B {\n  void b(A a) => a.a();\n}\n";
    let report = rig(main, lib1, None, false);
    assert!(
        code_rows(&report, "invalid_use_of_protected_member").is_empty(),
        "{report:?}"
    );
}

// —— invalid_use_of_visible_for_testing_member ——————————————————————

/// A `@visibleForTesting` member read from another library reports with
/// the declaring library's URI (upstream `test_getter`).
#[test]
fn testing_member_reports() {
    let lib1 = "import 'package:meta/meta.dart';\n\nclass A {\n  @visibleForTesting\n  int get a => 7;\n}\n";
    let main = "import 'lib1.dart';\n\nvoid f() {\n  A().a;\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_visible_for_testing_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The member 'a' can only be used within 'package:t/lib1.dart' or a test."
    );
    assert_eq!(rows[0].range, at(main, "A().a;", 4));
}

/// A named constructor's own annotation reports at the whole
/// `ConstructorName` with the joined display (upstream
/// `test_namedConstructor`: `A.forTesting`, twelve columns).
#[test]
fn testing_named_constructor_join() {
    let lib1 = "import 'package:meta/meta.dart';\n\nclass A {\n  @visibleForTesting\n  A.forTesting();\n}\n";
    let main = "import 'lib1.dart';\n\nvoid f() {\n  A.forTesting();\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_visible_for_testing_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The member 'A.forTesting' can only be used within 'package:t/lib1.dart' or a test."
    );
    let anchor = at(main, "A.forTesting();", 0);
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor.offset,
            length: "A.forTesting".len() as u32,
        }
    );
}

/// An unnamed constructor's annotation reports the class spelling alone
/// (upstream `test_unnamedConstructor`).
#[test]
fn testing_unnamed_constructor_reports() {
    let lib1 = "import 'package:meta/meta.dart';\n\nclass A {\n  @visibleForTesting\n  A();\n}\n";
    let main = "import 'lib1.dart';\n\nvoid f() {\n  A();\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_visible_for_testing_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The member 'A' can only be used within 'package:t/lib1.dart' or a test."
    );
    assert_eq!(rows[0].range, at(main, "A();", 0));
}

/// A top-level function's annotation reports at the bare reference
/// (upstream `test_topLevelFunction` — the bare-reference funnel).
#[test]
fn testing_top_level_function_reports() {
    let lib1 = "import 'package:meta/meta.dart';\n\n@visibleForTesting\nvoid f() {}\n";
    let main = "import 'lib1.dart';\n\nvoid g() {\n  f();\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_visible_for_testing_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The member 'f' can only be used within 'package:t/lib1.dart' or a test."
    );
    assert_eq!(rows[0].range, at(main, "f();", 0));
}

/// A setter write reports at the selector (upstream `test_setter` — the
/// write edge, which the read funnel never sees).
#[test]
fn testing_setter_write_reports() {
    let lib1 = "import 'package:meta/meta.dart';\n\nclass A {\n  @visibleForTesting\n  set b(int i) {}\n}\n";
    let main = "import 'lib1.dart';\n\nvoid f() {\n  A().b = 6;\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_visible_for_testing_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The member 'b' can only be used within 'package:t/lib1.dart' or a test."
    );
    assert_eq!(rows[0].range, at(main, "A().b = 6;", 4));
}

// —— invalid_use_of_visible_for_overriding_member —————————————————

/// A `super.x` access outside the member that overrides `x` reports; the
/// override's own `super.x` is silent (upstream `test_overriding_getter`,
/// `test_getter`).
#[test]
fn overriding_member_reports_outside_the_override() {
    let lib1 = "import 'package:meta/meta.dart';\n\nclass A {\n  @visibleForOverriding\n  int get g => 0;\n}\n";
    let main = "import 'lib1.dart';\n\nclass B extends A {\n  @override\n  int get g => super.g + 1;\n\n  int get x => super.g + 1;\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_visible_for_overriding_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The member 'g' can only be used for overriding."
    );
    assert_eq!(rows[0].range, at_last(main, "super.g", 6));
}

/// A plain cross-library field read reports (upstream `test_field`).
#[test]
fn overriding_field_reports() {
    let lib1 =
        "import 'package:meta/meta.dart';\n\nclass A {\n  @visibleForOverriding\n  int g = 0;\n}\n";
    let main = "import 'lib1.dart';\n\nint m(A a) {\n  return a.g;\n}\n";
    let report = rig(main, lib1, None, false);
    let rows = code_rows(&report, "invalid_use_of_visible_for_overriding_member");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The member 'g' can only be used for overriding."
    );
    assert_eq!(rows[0].range, at(main, "a.g;", 2));
}

/// A same-library use is exempt (upstream `test_sameLibrary`).
#[test]
fn overriding_member_same_library_silent() {
    let lib1 = "";
    let main = "import 'package:meta/meta.dart';\n\nclass A {\n  @visibleForOverriding\n  int g = 0;\n}\n\nint m(A a) => a.g;\n";
    let report = rig(main, lib1, None, false);
    assert!(
        code_rows(&report, "invalid_use_of_visible_for_overriding_member").is_empty(),
        "{report:?}"
    );
}

// —— type_annotation_deferred_class ————————————————————————————————
//
// Every fixture imports the library under test through the deferred
// prefix `a`, so `a.A` is the `.isDeferred` shape.

/// A variable declaration's type annotation reports at the `NamedType`
/// (`a.A` — the three columns the oracle anchors; upstream
/// `test_variableDeclarationList`).
#[test]
fn deferred_class_variable_annotation_reports() {
    let lib1 = "class A {}\n\nclass C<E> {}\n";
    let main = "import 'lib1.dart' deferred as a;\n\nvoid f() {\n  a.A v = a.A();\n}\n";
    let report = rig(main, lib1, Some("a"), true);
    let rows = code_rows(&report, "type_annotation_deferred_class");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Error);
    assert_eq!(
        rows[0].message,
        "The deferred type 'a.A' can't be used in a declaration, cast, or type test."
    );
    let anchor = at(main, "a.A v", 0);
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor.offset,
            length: 3,
        }
    );
}

/// The `is` and `as` operands report (upstream `test_isExpression`,
/// `test_asExpression`).
#[test]
fn deferred_class_is_and_as_operands_report() {
    let lib1 = "class A {}\n";
    let main = "import 'lib1.dart' deferred as a;\n\nvoid f(Object v) {\n  if (v is a.A) {}\n  v as a.A;\n}\n";
    let report = rig(main, lib1, Some("a"), true);
    let rows = code_rows(&report, "type_annotation_deferred_class");
    assert_eq!(rows.len(), 2, "{report:?}");
    let is_anchor = at(main, "v is a.A", 5);
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: is_anchor.offset,
            length: 3,
        }
    );
    let as_anchor = at(main, "v as a.A", 5);
    assert_eq!(
        rows[1].range,
        SourceRange {
            offset: as_anchor.offset,
            length: 3,
        }
    );
}

/// A type argument reports, and the outer named type spans its group
/// (upstream `test_typeArgumentList`; the anchor is the inner
/// `NamedType`).
#[test]
fn deferred_class_type_argument_reports() {
    let lib1 = "class A {}\n\nclass C<E> {}\n";
    let main = "import 'lib1.dart' deferred as a;\n\nC<a.A> c = C();\n";
    let report = rig(main, lib1, Some("a"), true);
    let rows = code_rows(&report, "type_annotation_deferred_class");
    assert_eq!(rows.len(), 1, "{report:?}");
    let anchor = at(main, "C<a.A> c", 2);
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor.offset,
            length: 3,
        }
    );
}

/// A type parameter's bound reports (upstream `test_typeParameter_bound`).
#[test]
fn deferred_class_type_parameter_bound_reports() {
    let lib1 = "class A {}\n";
    let main = "import 'lib1.dart' deferred as a;\n\nclass C<E extends a.A> {}\n";
    let report = rig(main, lib1, Some("a"), true);
    let rows = code_rows(&report, "type_annotation_deferred_class");
    assert_eq!(rows.len(), 1, "{report:?}");
    let anchor = at(main, "E extends a.A", 10);
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor.offset,
            length: 3,
        }
    );
}

/// A return type reports with the `?` suffix inside the node span
/// (upstream `test_functionDeclaration_returnType`: four columns).
#[test]
fn deferred_class_nullable_return_type_spans_the_question() {
    let lib1 = "class A {}\n";
    let main = "import 'lib1.dart' deferred as a;\n\na.A? f() => null;\n";
    let report = rig(main, lib1, Some("a"), true);
    let rows = code_rows(&report, "type_annotation_deferred_class");
    assert_eq!(rows.len(), 1, "{report:?}");
    let anchor = at(main, "a.A? f()", 0);
    assert_eq!(
        rows[0].range,
        SourceRange {
            offset: anchor.offset,
            length: 4,
        }
    );
}

/// A supertype clause is never checked (no `_checkForTypeAnnotation…`
/// visitor site reaches `extends`).
#[test]
fn deferred_class_supertype_clause_silent() {
    let lib1 = "class A {}\n";
    let main = "import 'lib1.dart' deferred as a;\n\nclass B extends a.A {}\n";
    let report = rig(main, lib1, Some("a"), true);
    assert!(
        code_rows(&report, "type_annotation_deferred_class").is_empty(),
        "{report:?}"
    );
}

/// A function type's return type is never checked (upstream
/// `visitFunctionTypeAlias` :1304-1314 visits no type annotation).
#[test]
fn deferred_class_function_type_return_silent() {
    let lib1 = "class A {}\n";
    let main = "import 'lib1.dart' deferred as a;\n\ntypedef F = a.A Function();\n";
    let report = rig(main, lib1, Some("a"), true);
    assert!(
        code_rows(&report, "type_annotation_deferred_class").is_empty(),
        "{report:?}"
    );
}

/// A prefix whose import is not deferred reports nothing
/// (`NamedType.isDeferred` reads the fragment flag).
#[test]
fn deferred_class_plain_prefix_silent() {
    let lib1 = "class A {}\n";
    let main = "import 'lib1.dart' as a;\n\nvoid f() {\n  a.A v = a.A();\n}\n";
    let report = rig(main, lib1, Some("a"), false);
    assert!(
        code_rows(&report, "type_annotation_deferred_class").is_empty(),
        "{report:?}"
    );
}
