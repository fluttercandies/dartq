//! The #2413 pins: cross-library typedef call resolution (N5+N9) and the
//! callable-class invocation return type (R4).
//!
//! N5+N9: a `typedef` declared in another library file must still resolve its
//! callable signature at the use site — `joiner(x)` counts positionals against
//! the alias RHS, and `takes((String s) => true)` checks the argument's
//! function type against the alias's declared formals. The N1 probe showed the
//! oracle itself is silent on `bool.fromEnvironment` (Dart 3.13.1 folds to the
//! `defaultValue` only through `DeclaredVariables`, and `dead_code` is a hint
//! the machine face does not emit), so there is no pin for it.
//!
//! R4: a class instance with a `call` method invokes through `.call` — the
//! send's return type is the call method's declared return, and the
//! INVALID_ASSIGNMENT span covers the whole invocation `c(3)`, not just `c`.
//!
//! Oracle provenance: probed against `dart analyze` (Dart 3.13.1, fvm 3.47.1)
//! on the `/tmp/probe2413` fixtures.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `main_source` as `main.dart` importing `lib.dart`, a two-file
/// library — the typedef lives in `lib.dart`, the call sites in `main.dart`.
fn report_two_file(main_source: &str, lib_source: &str) -> FileReport {
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
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.seed_implicit_core(
        lib_main,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "void",
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

/// Single-file rig for the callable-class face (R4).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "void",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
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

const TYPEDEF_LIB: &str = "typedef Callback<T, R> = R Function(T value);\n\
                           typedef Predicate<T> = Callback<T, bool>;\n\
                           typedef FailureJoiner = int Function(int first, int second);\n";

/// N9: a non-generic alias declared in another file counts positionals at the
/// call — `joiner(1)` is `not_enough_positional_arguments` (oracle: 2 expected).
#[test]
fn foreign_alias_positional_count_is_checked() {
    let main = "import 'lib.dart';\n\
                int run(FailureJoiner joiner) {\n\
                \x20 return joiner(1);\n\
                }\n";
    let names = code_names(&report_two_file(main, TYPEDEF_LIB));
    assert!(
        names.contains(&"not_enough_positional_arguments"),
        "{names:?}"
    );
}

/// N5, parameterized cell — the element model now captures a
/// parameterized alias's RHS (`var_ty` in the written shape, its
/// parameter spellings as bare leaves), and the use-site substitution
/// face rewrites `Predicate<String>` to the aliased `bool Function(String)`
/// at the formal's bind — so `pred(42)` counts against the instantiated
/// formals and reports `argument_type_not_assignable` at the argument,
/// exactly the oracle's verdict (probed: /tmp/probe2410, dart analyze
/// Dart 3.13.1, single- and two-file fixtures alike).
#[test]
fn foreign_parameterized_alias_argument_is_checked() {
    let main = "import 'lib.dart';\n\
                bool run(Predicate<String> pred) {\n\
                \x20 return pred(42);\n\
                }\n";
    let report = report_two_file(main, TYPEDEF_LIB);
    let names = code_names(&report);
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "parameterized-alias formals unresolved: {names:?}"
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .expect("argument row");
    assert_eq!(row.range.length, 2, "the span covers the `42` argument");
}

/// N5, lambda-argument cell — documented under-report: a function literal
/// leaves a multi-row window the row recovery cannot attribute (pushed as the
/// zero-span `Unknown` face), and the only banked signature
/// (`closure_capture_end`) renders `return_ty: Ty::Void` for the
/// `could_not_infer` face — not the inferred return the function-subtype
/// decision needs. Wiring it into `arg_sigs` misjudged every non-void formal
/// (the `argument_position_literal_never_leaks` pin proved `() => DB()` into
/// `Str Function()` would false-positive), so this cell stays silent until the
/// closure's real inferred return reaches the subtype face. The oracle emits
/// `argument_type_not_assignable` on `takes((String s) => true)`.
#[test]
fn foreign_alias_formal_lambda_argument_is_an_unresolved_under_report() {
    let main = "import 'lib.dart';\n\
                void takes(FailureJoiner f) {}\n\
                void run() {\n\
                \x20 takes((String s) => true);\n\
                }\n";
    let names = code_names(&report_two_file(main, TYPEDEF_LIB));
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "closure inferred-return wired — flip this pin: {names:?}"
    );
}

/// R4: an instance with a `call` method invoked as `c(3)` types the send as
/// the call method's return; assigning it to `int` reports
/// `invalid_assignment` anchored on the whole `c(3)` invocation (col of `c`,
/// length 4).
#[test]
fn callable_instance_return_types_the_invocation() {
    let source = "class Converter {\n\
                  \x20 String call(int x) => 'v';\n\
                  }\n\
                  int run() {\n\
                  \x20 var c = Converter();\n\
                  \x20 int bad = c(3);\n\
                  \x20 return bad;\n\
                  }\n";
    let report = report(source);
    let names = code_names(&report);
    assert!(names.contains(&"invalid_assignment"), "{names:?}");
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "invalid_assignment")
        .expect("invalid_assignment row");
    assert_eq!(row.range.length, 4, "span should cover `c(3)`");
}
