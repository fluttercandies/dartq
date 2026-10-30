//! The #2993 pins: the `invalid_annotation_target`,
//! `unnecessary_set_literal`, `unnecessary_final`, and
//! `missing_required_param` WARNING faces (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/annotation_verifier.dart
//! `_checkKinds` :373-401, best_practices_verifier.dart
//! `_checkForUnnecessarySetLiteral` :1353-1391 and
//! `_checkFinalParameter` :932-940, required_parameters_verifier.dart
//! `_check` :120-196).
//!
//! Positive pins mirror the upstream `InvalidAnnotationTargetTest` /
//! `BestPracticesVerifierTest` / `RequiredParametersVerifierTest`
//! fixtures verbatim — the oracle's own expected rows (message, anchor
//! column/length) — plus the silent negatives (an allowed target, a
//! non-set arrow body, a statement `final`, a supplied named argument).

use super::*;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_diagnostics::Severity;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The package:meta stand-ins the annotation faces resolve through
/// (the conformance mocks' declarations, pared to the names the faces
/// read): `library meta;` carries the `required` getter and `Required`
/// class, `library meta_meta;` carries `Target`.
const META_MOCK: &str = r#"library meta;

const Required required = Required();

class Required {
  final String reason;
  const Required([this.reason = '']);
}
"#;

const META_META_MOCK: &str = r#"library meta_meta;

class Target {
  const Target(this.kinds);
  final List<Object> kinds;
}
"#;

fn code_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .collect()
}

/// The single-unit rig: no imports, no mocks — the faces that resolve
/// nothing (the set-literal, parameter-final, and `@override` faces).
fn unit_report(main: &str) -> FileReport {
    rig_report(main, &[])
}

/// The package:meta rig: the main unit imports both mocks (the
/// annotation-target and required-parameter faces resolve their stars
/// through the same edges the oracle does).
fn meta_report(main: &str) -> FileReport {
    rig_report(
        main,
        &[
            ("package:meta/meta.dart", "meta", META_MOCK),
            ("package:meta/meta_meta.dart", "meta_meta", META_META_MOCK),
        ],
    )
}

/// The shared rig: main plus one mock library per `(uri, library name,
/// source)` triple, each imported plain by the main unit.
fn rig_report(main: &str, mocks: &[(&str, &str, &str)]) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan_fixture(main, file);
    let elements = crate::collector::collect(main, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.unit(lib, file);
    let mut mock_elements = Vec::new();
    for (uri, lib_name, source) in mocks {
        let mock_file = files.intern(Path::new(uri));
        let mock_lib = builder.intern(uri);
        builder.library_name(mock_lib, lib_name);
        let elements = crate::collector::collect(source, &scan_fixture(source, mock_file).tokens, features, &strings);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(mock_lib, name, mock_file);
        }
        builder.unit(mock_lib, mock_file);
        builder.import(
            lib,
            ImportEdge {
                target: Some(mock_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        mock_elements.push((mock_file, elements));
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    for (mock_file, elements) in mock_elements {
        index.insert(mock_file, elements);
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

// —— invalid_annotation_target ————————————————————————————————————————

/// A `@Target`-carrying class's constructor annotation on a mixin
/// reports the unnamed-constructor display at the name token (upstream
/// `test_classType_mixin`).
#[test]
fn iat_ctor_annotation_on_mixin() {
    let main = "import 'package:meta/meta_meta.dart';\n\n@Target({TargetKind.classType})\nclass A {\n  const A();\n}\n\n@A()\nmixin M {}\n";
    let report = meta_report(main);
    let rows = code_rows(&report, "invalid_annotation_target");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Warning);
    assert_eq!(
        rows[0].message,
        "The annotation 'A.new' can only be used on classes."
    );
    let at = main.find("@A()").expect("fixture anchor") + 1;
    assert_eq!(rows[0].range, SourceRange { offset: at as u32, length: 1 });
}

/// A `@Target`-carrying top-level constant's bare annotation on a
/// top-level variable reports the bare-head display through the
/// const-creation face (upstream
/// `test_classType_topLevelVariable_topLevelConstant`).
#[test]
fn iat_constant_annotation_on_top_level_variable() {
    let main = "import 'package:meta/meta_meta.dart';\n\n@Target({TargetKind.classType})\nclass A {\n  const A();\n}\n\nconst a = A();\n\n@a\nint x = 0;\n";
    let report = meta_report(main);
    let rows = code_rows(&report, "invalid_annotation_target");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The annotation 'a' can only be used on classes."
    );
    let at = main.find("@a\n").expect("fixture anchor") + 1;
    assert_eq!(rows[0].range, SourceRange { offset: at as u32, length: 1 });
}

/// The dart:core `@override` face on a top-level variable: the hardcoded
/// allow-list, the Oxford-comma target list (upstream
/// `test_override_topLevelField`).
#[test]
fn iat_override_on_top_level_variable() {
    let main = "@override\nint x = 0;\n";
    let report = unit_report(main);
    let rows = code_rows(&report, "invalid_annotation_target");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "The annotation 'override' can only be used on fields, getters, methods, or setters."
    );
    let at = main.find("@override").expect("fixture anchor") + 1;
    assert_eq!(rows[0].range, SourceRange { offset: at as u32, length: 8 });
}

/// An allowed target is silent: the same `@A()` on a class
/// (upstream `test_classType_class`, expected `-`).
#[test]
fn iat_allowed_target_silent() {
    let main = "import 'package:meta/meta_meta.dart';\n\n@Target({TargetKind.classType})\nclass A {\n  const A();\n}\n\n@A()\nclass B {}\n";
    let report = meta_report(main);
    assert!(
        code_rows(&report, "invalid_annotation_target").is_empty(),
        "{report:?}"
    );
}

// —— unnecessary_set_literal ——————————————————————————————————————————

/// A void-returning top-level function's arrow set-literal body reports
/// at the braces (upstream `BestPracticesVerifierTest`
/// `_checkForUnnecessarySetLiteral`'s declared-return face).
#[test]
fn usl_declared_return_face() {
    let main = "void f() => {1};\n";
    let report = unit_report(main);
    let rows = code_rows(&report, "unnecessary_set_literal");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Warning);
    assert_eq!(
        rows[0].message,
        "Braces unnecessarily wrap this expression in a set literal."
    );
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try removing the set literal around the expression.")
    );
    let at = main.find("{1}").expect("fixture anchor") as u32;
    assert_eq!(rows[0].range, SourceRange { offset: at, length: 3 });
}

/// A closure argument under a void-returning formal type reports at its
/// braces (the `correspondingParameter` face).
#[test]
fn usl_closure_argument_face() {
    let main = "void g(void Function() cb) {}\nvoid f() { g(() => {1}); }\n";
    let report = unit_report(main);
    let rows = code_rows(&report, "unnecessary_set_literal");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "Braces unnecessarily wrap this expression in a set literal."
    );
    let at = main.rfind("{1}").expect("fixture anchor") as u32;
    assert_eq!(rows[0].range, SourceRange { offset: at, length: 3 });
}

/// A non-void declared return is silent (a set literal is the point of
/// the return), and a void-returning map literal is silent (`isSet`).
#[test]
fn usl_non_void_and_map_silent() {
    let report = unit_report("Set<int> f() => {1};\nvoid m() => {1: 2};\n");
    assert!(
        code_rows(&report, "unnecessary_set_literal").is_empty(),
        "{report:?}"
    );
}

// —— unnecessary_final ————————————————————————————————————————————————

/// A formal parameter's `final` reports at the keyword token (upstream
/// `_checkFinalParameter`, `BestPracticesVerifierTest`).
#[test]
fn uf_final_parameter() {
    let main = "void f(final int x) {}\n";
    let report = unit_report(main);
    let rows = code_rows(&report, "unnecessary_final");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Warning);
    assert_eq!(
        rows[0].message,
        "The keyword 'final' isn't necessary because the parameter is implicitly 'final'."
    );
    assert_eq!(
        rows[0].correction.as_deref(),
        Some("Try removing the 'final'.")
    );
    let at = main.find("final").expect("fixture anchor") as u32;
    assert_eq!(rows[0].range, SourceRange { offset: at, length: 5 });
}

/// Statement-level `final`s are silent: a block-local declaration and a
/// for-head's loop variable are not parameters.
#[test]
fn uf_statement_finals_silent() {
    let report = unit_report("void f() {\n  final int x = 1;\n  for (final y in []) {}\n}\n");
    assert!(
        code_rows(&report, "unnecessary_final").is_empty(),
        "{report:?}"
    );
}

// —— missing_required_param ———————————————————————————————————————————

/// An un-supplied `@required` named parameter reports the plain text at
/// the callee's name token (upstream `RequiredParametersVerifierTest`).
#[test]
fn mrp_plain_required() {
    let main = "import 'package:meta/meta.dart';\n\nvoid f({@required int x}) {}\n\nvoid main() {\n  f();\n}\n";
    let report = meta_report(main);
    let rows = code_rows(&report, "missing_required_param");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].severity, Severity::Warning);
    assert_eq!(rows[0].message, "The parameter 'x' is required.");
    let at = main.rfind("f()").expect("fixture anchor") as u32;
    assert_eq!(rows[0].range, SourceRange { offset: at, length: 1 });
}

/// A `@Required('reason')` class star reports the with-details text.
#[test]
fn mrp_required_with_details() {
    let main = "import 'package:meta/meta.dart';\n\nvoid f({@Required('why not') int x}) {}\n\nvoid main() {\n  f();\n}\n";
    let report = meta_report(main);
    let rows = code_rows(&report, "missing_required_param");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "The parameter 'x' is required. why not.");
}

/// A supplied named argument is silent.
#[test]
fn mrp_supplied_argument_silent() {
    let main = "import 'package:meta/meta.dart';\n\nvoid f({@required int x}) {}\n\nvoid main() {\n  f(x: 1);\n}\n";
    let report = meta_report(main);
    assert!(
        code_rows(&report, "missing_required_param").is_empty(),
        "{report:?}"
    );
}
