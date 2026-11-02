//! The #3353 pins: the `deprecated_*` functionality faces that had no emit
//! site at all — `deprecated_extend`, `deprecated_implement`,
//! `deprecated_mixin`, `deprecated_subclass`, `deprecated_instantiate`,
//! `deprecated_optional`, and the pre-3.0 `deprecated_subtype_of_function`
//! arms (upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! deprecated_functionality_verifier.dart and best_practices_verifier.dart
//! `_checkForBadFunctionUse` :3157-3203).
//!
//! Every positive pins the ORACLE's own row: the case corpus's message text,
//! code, and anchor column/length
//! (`conformance` cases `deprecated_extend/DeprecatedExtendTest/
//! test_annotatedClass`, `.../test_classTypeAlias`, the
//! `DeprecatedImplementTest`/`DeprecatedMixinTest`/`DeprecatedSubclassTest`/
//! `DeprecatedInstantiateTest` families, `DeprecatedOptionalTest/
//! test_argumentOmitted*`, and `DeprecatedExtendsFunctionTest/
//! test_core_language219`) — reproduced here over a two-library rig, because
//! the declaration faces carry the oracle's same-library exemption
//! (`element.library == _currentLibrary`) and therefore need a cross-library
//! target.
//!
//! The silent negatives pin the gates: same-library targets, the `use` kind
//! (a plain `@deprecated` never answers a `with kind` test), an alias-typed
//! creation (a typedef is no `InterfaceElement`), an argument that binds the
//! annotated formal, and the modern-language `Function` posture (the
//! class-modifiers feature turns `extends Function` into the outside-library
//! error instead).

use super::*;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The `dart:core` names the tag / `Function` arms resolve through: the two
/// deprecation tags, the built-in `Function` type, and a non-tag control.
const CORE_FACE: &[&str] = &["deprecated", "Deprecated", "Function", "Object"];

/// The dependency library's canonical URI — the edge the analyzed unit's
/// `import` resolves through.
const DEP_URI: &str = "package:s/kit.dart";

fn code_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .collect()
}

/// The oracle anchor `(offset, length)` of `needle` in an ASCII fixture
/// (byte offsets are UTF-16 offsets there).
fn anchor(source: &str, needle: &str, length: u32) -> (u32, u32) {
    let offset = source
        .find(needle)
        .unwrap_or_else(|| panic!("fixture does not spell {needle:?}: {source:?}"));
    (offset as u32, length)
}

/// The cross-library rig: `main` (this unit) imports `dependency` (declared
/// in another library, so the declaration faces' same-library exemption does
/// not swallow the row), both carrying the engine's implicit `dart:core`
/// edge. `legacy` records the unit's language version as pre-3.0 — the
/// `class_modifiers` gate `_checkForBadFunctionUse` opens with.
fn report(main: &str, dependency: &str, legacy: bool) -> FileReport {
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
    let dep_elements = crate::collector::collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:app/main.dart");
    let dep_lib = builder.intern(DEP_URI);
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
    let mut modifiers = LibraryClassModifiers::default();
    if legacy {
        modifiers.insert(lib, false);
    }
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
        &modifiers,
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

// —— the declaration faces ————————————————————————————————————————————————

/// `class Bar extends Foo` over `@Deprecated.extend() class Foo` — the
/// `test_annotatedClass` row, anchored at the written `NamedType`.
#[test]
fn extend_face_reports_at_the_supertype() {
    const MAIN: &str = "import 'package:s/kit.dart';\nclass Bar extends Foo {}\n";
    let report = report(
        MAIN,
        "@Deprecated.extend()\nclass Foo {}",
        false,
    );
    let rows = code_rows(&report, "deprecated_extend");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "Extending 'Foo' is deprecated.");
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try removing the 'extends' clause.")
    );
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "Foo {}", 3)
    );
}

/// `class Bar implements Foo` — the `test_annotatedClass` implement row.
#[test]
fn implement_face_reports_at_the_interface() {
    const MAIN: &str = "import 'package:s/kit.dart';\nclass Bar implements Foo {}\n";
    let report = report(MAIN, "@Deprecated.implement()\nclass Foo {}", false);
    let rows = code_rows(&report, "deprecated_implement");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "Implementing 'Foo' is deprecated.");
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try removing 'Foo' from the 'implements' clause.")
    );
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "Foo {}", 3)
    );
}

/// `class Bar with Foo` — the `test_class` mixin row.
#[test]
fn mixin_face_reports_at_the_mixin_type() {
    const MAIN: &str = "import 'package:s/kit.dart';\nclass Bar with Foo {}\n";
    let report = report(MAIN, "@Deprecated.mixin()\nclass Foo {}", false);
    let rows = code_rows(&report, "deprecated_mixin");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "Mixing in 'Foo' is deprecated.");
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try removing 'Foo' from the 'with' clause.")
    );
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "Foo {}", 3)
    );
}

/// `@Deprecated.subclass()` on the superclass — the `test_extendsClass` row.
#[test]
fn subclass_face_reports_the_extends_arm() {
    const MAIN: &str = "import 'package:s/kit.dart';\nclass Bar extends Foo {}\n";
    let report = report(MAIN, "@Deprecated.subclass()\nclass Foo {}", false);
    let rows = code_rows(&report, "deprecated_subclass");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "Subclassing 'Foo' is deprecated.");
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try removing the 'extends' clause, or removing 'Foo' from the 'implements' clause.")
    );
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "Foo {}", 3)
    );
}

/// The `with` list's second reading (`_checkForDeprecatedSubclass`'s twin
/// call from `classDeclaration`): a `subclass`-kind mixin reports the
/// `deprecated_subclass` code and never `deprecated_mixin`.
#[test]
fn subclass_face_reads_the_with_list() {
    const MAIN: &str = "import 'package:s/kit.dart';\nclass Bar with Foo {}\n";
    let report = report(MAIN, "@Deprecated.subclass()\nclass Foo {}", false);
    let rows = code_rows(&report, "deprecated_subclass");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "Subclassing 'Foo' is deprecated.");
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "Foo {}", 3)
    );
    assert!(
        code_rows(&report, "deprecated_mixin").is_empty(),
        "the subclass kind never answers the mixin arm: {report:?}"
    );
}

/// The same-library exemption (`element.library == _currentLibrary`): the
/// annotated superclass declared in the analyzed unit itself stays silent —
/// the `test_insideLibrary` face.
#[test]
fn a_same_library_target_is_exempt() {
    let report = report(
        "class Bar extends Foo {}\n@Deprecated.extend()\nclass Foo {}",
        "",
        false,
    );
    assert!(
        code_rows(&report, "deprecated_extend").is_empty(),
        "{report:?}"
    );
}

/// A plain `@deprecated` tag answers the `use` kind only: no `with kind`
/// face fires (the `test_noAnnotation`-adjacent silent direction).
#[test]
fn the_plain_tag_answers_no_kind_face() {
    let report = report(
        "import 'package:s/kit.dart';\nclass Bar extends Foo {}",
        "@deprecated\nclass Foo {}",
        false,
    );
    assert!(
        code_rows(&report, "deprecated_extend").is_empty()
            && code_rows(&report, "deprecated_subclass").is_empty(),
        "{report:?}"
    );
}

// —— the creation face ————————————————————————————————————————————————————

/// `Foo()` over `@Deprecated.instantiate() class Foo` — the
/// `test_annotatedClass` row at the `ConstructorName`.
#[test]
fn instantiate_face_reports_at_the_constructor_name() {
    const MAIN: &str = "import 'package:s/kit.dart';\nvoid g() { Foo x = Foo(); }\n";
    let report = report(MAIN, "@Deprecated.instantiate()\nclass Foo {}", false);
    let rows = code_rows(&report, "deprecated_instantiate");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "Instantiating 'Foo' is deprecated.");
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try instantiating a non-abstract class.")
    );
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "Foo()", 3)
    );
}

/// A typedef head is no `InterfaceElement` (`test_annotatedClass_typedef`
/// expects no row): the publication resolves the alias through to its
/// right-hand name, and the written-head guard restores the oracle's answer.
#[test]
fn instantiate_face_ignores_an_alias_head() {
    let report = report(
        "import 'package:s/kit.dart';\nvoid g() { F2 x = F2(); }",
        "@Deprecated.instantiate()\nclass Foo {}\ntypedef F2 = Foo;",
        false,
    );
    assert!(
        code_rows(&report, "deprecated_instantiate").is_empty(),
        "{report:?}"
    );
}

// —— the omission face ————————————————————————————————————————————————————

/// `f()` with an omitted `@Deprecated.optional()` formal — the
/// `test_argumentOmitted` / `test_argumentOmitted_named` row at the callee.
#[test]
fn optional_face_reports_an_omitted_named_formal() {
    const MAIN: &str = "import 'package:s/kit.dart';\nvoid g() { f(); }\n";
    let report = report(MAIN, "void f({@Deprecated.optional() int? p}) {}", false);
    let rows = code_rows(&report, "deprecated_optional");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "Omitting an argument for the 'p' parameter is deprecated."
    );
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try passing an argument for 'p'.")
    );
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "f()", 1)
    );
}

/// The member arm (`test_argumentOmitted_methodInvocation`): the row anchors
/// the selector, not the receiver.
#[test]
fn optional_face_reports_a_member_call() {
    const MAIN: &str = "import 'package:s/kit.dart';\nvoid g(C c) { c.m(); }\n";
    let report = report(
        MAIN,
        "class C { void m({@Deprecated.optional() int? p}) {} }",
        false,
    );
    let rows = code_rows(&report, "deprecated_optional");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "m()", 1)
    );
}

/// Supplying the annotated NAMED formal silences the face
/// (`test_argumentGiven_named`): only the omitting call reports.
#[test]
fn optional_face_respects_a_supplied_argument() {
    let report = report(
        "import 'package:s/kit.dart';\nvoid g() { f(p: 1); f(); }",
        "void f({@Deprecated.optional() int? p}) {}",
        false,
    );
    // Only the last call omits the formal.
    let rows = code_rows(&report, "deprecated_optional");
    assert_eq!(rows.len(), 1, "{report:?}");
}

/// The positional arm (`test_argumentGiven`): an optional-positional formal
/// bound by the argument reports nothing, while the omitting call still does.
#[test]
fn optional_face_respects_a_supplied_positional() {
    let report = report(
        "import 'package:s/kit.dart';\nvoid g() { f(1); }",
        "void f([@Deprecated.optional() int? p]) {}",
        false,
    );
    assert!(
        code_rows(&report, "deprecated_optional").is_empty(),
        "{report:?}"
    );
}

// —— the pre-3.0 `Function` arms ——————————————————————————————————————————

/// `class A extends Function` under a pre-3.0 language version — the
/// `test_core_language219` row (`Extending 'Function' is deprecated.`), the
/// arm `_checkForBadFunctionUse` gates on the class-modifiers feature.
#[test]
fn function_extends_face_reports_before_class_modifiers() {
    const MAIN: &str = "class A extends Function {}\n";
    let report = report(MAIN, "", true);
    let rows = code_rows(&report, "deprecated_subtype_of_function");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "Extending 'Function' is deprecated.");
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try removing 'Function' from the 'extends' clause.")
    );
    assert_eq!(
        (rows[0].range.offset, rows[0].range.length),
        anchor(MAIN, "Function {}", 8)
    );
}

/// The same source at the modern language version: the feature gate answers
/// no row (`test_core` expects the outside-library error instead).
#[test]
fn function_extends_face_is_silent_with_class_modifiers() {
    let report = report("class A extends Function {}\n", "", false);
    assert!(
        code_rows(&report, "deprecated_subtype_of_function").is_empty(),
        "{report:?}"
    );
}
