//! The #2425 pins: the function-involved cross-family assignability face.
//!
//! #2417's projection made a foreign constructor's `Ty::Function` formal
//! readable in the diagnosing pool, which exposed an under-report: the
//! argument walk asked [`Engine::assignable`] and the matrix answered
//! `None` for every pair carrying a function shape on one side and an
//! interface type on the other — `Ty::assignable_from`'s cross-family arm
//! suppresses, and the super-invocation walk's lattice fallback only
//! carries the named-vs-named pair. `super('x')` against a foreign
//! `int Function(int)` formal stayed silent where the oracle reports
//! `argument_type_not_assignable`; the same-file twin was equally silent,
//! so the gap was the assignability face, not the projection.
//!
//! The fix mirrors upstream `isAssignableTo` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:891-930): a
//! function source subtypes exactly the `Object` and `Function` heads
//! among interface targets (subtype.dart's tail `return false` is
//! decisive); an interface or record source subtypes a function target in
//! no case — except the 'call' tearoff arm (type_system.dart:906-916): a
//! resolved class whose `call` method's own signature subtypes the target
//! assigns (the oracle accepts a callable class instance where a function
//! type is expected, and rejects one whose `call` signature disagrees).
//!
//! Oracle provenance: /tmp/probe2425 — the two-file repro
//! (`class B { B(int Function(int) f); }` in base.dart, `class C extends
//! B { C() : super('x'); }` in main.dart) reports
//! `argument_type_not_assignable` on `dart analyze` 3.13.1 and was silent
//! on dartq pre-fix; the same-file twin, the
//! function-argument-vs-`int`-formal twin, and the callable-class twins
//! behave as pinned here on the same oracle.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `main_source` as `main.dart` importing `lib.dart` — the
/// ticket_2417 two-file harness, reused so the foreign-pool formals ride
/// the same path the projection fix landed on. Same-file cases put the
/// whole fixture in `main_source` (`lib.dart` stays a filler the harness
/// still links).
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The ticket repro (foreign pool): a `String` literal against the
/// projected `int Function(int)` formal reports — the cross-family face
/// decides the rejection instead of suppressing it.
#[test]
fn super_invocation_foreign_function_formal_mismatch_reports() {
    let lib = "class B {\n\
               \x20 B(int Function(int) f);\n\
               }\n";
    let main = "import 'lib.dart';\n\
                class C extends B {\n\
                \x20 C() : super('x');\n\
                }\n";
    assert_eq!(
        code_names(&report_two_file(main, lib)),
        ["argument_type_not_assignable"]
    );
}

/// The matched twin: a proper `(int) => int` closure against the foreign
/// function formal keeps the #2417 silence. The function-expression
/// inference face is not modeled, so the closure argument reaches the walk
/// as `Ty::UNKNOWN` and the unknown suppression holds — the same silence
/// the oracle reaches through acceptance.
#[test]
fn super_invocation_foreign_function_formal_match_stays_silent() {
    let lib = "class B {\n\
               \x20 B(int Function(int) f);\n\
               }\n";
    let main = "import 'lib.dart';\n\
                class C extends B {\n\
                \x20 C() : super((int x) => x);\n\
                }\n";
    assert_eq!(code_names(&report_two_file(main, lib)), Vec::<&str>::new());
}

/// The same-file twin of the repro: the gap was never the projection —
/// the walk decides the same rejection with no pool boundary in the way.
#[test]
fn super_invocation_same_file_function_formal_mismatch_reports() {
    let main = "class B {\n\
                \x20 B(int Function(int) f);\n\
                }\n\
                \n\
                class C extends B {\n\
                \x20 C() : super('x');\n\
                }\n";
    assert_eq!(
        code_names(&report_two_file(main, "class Z {}\n")),
        ["argument_type_not_assignable"]
    );
}

/// The same-file matched twin stays silent (the same unknown-argument
/// suppression as the foreign twin).
#[test]
fn super_invocation_same_file_function_formal_match_stays_silent() {
    let main = "class B {\n\
                \x20 B(int Function(int) f);\n\
                }\n\
                \n\
                class C extends B {\n\
                \x20 C() : super((int x) => x);\n\
                }\n";
    assert_eq!(
        code_names(&report_two_file(main, "class Z {}\n")),
        Vec::<&str>::new()
    );
}

/// The mirrored direction: a function source against an `int` formal —
/// the subtype question is decisive the same way (only `Object` and
/// `Function` are interface supertypes of a function type), so the
/// rejection reports. A function *expression* argument still reaches the
/// walk as `Ty::UNKNOWN` (the inference face is not modeled — the
/// residual under-report), so the pinned source is a function-typed
/// variable, whose read row carries its declared type.
#[test]
fn super_invocation_function_argument_to_int_formal_reports() {
    let main = "class B {\n\
                \x20 B(int i);\n\
                }\n\
                \n\
                class C extends B {\n\
                \x20 C(int Function() g) : super(g);\n\
                }\n";
    assert_eq!(
        code_names(&report_two_file(main, "class Z {}\n")),
        ["argument_type_not_assignable"]
    );
}

/// The 'call' tearoff arm (type_system.dart:906-916): a class with a
/// compatible `call` method assigns into a function-typed formal — the
/// oracle accepts the callable class instance (probed with the value as a
/// `Caller`-typed parameter, whose read row carries the declared type),
/// so the rejection face must stay silent here.
#[test]
fn callable_class_into_function_formal_stays_silent() {
    let main = "class Caller {\n\
                \x20 void call() {}\n\
                }\n\
                \n\
                class B {\n\
                \x20 B(void Function() f);\n\
                }\n\
                \n\
                class C extends B {\n\
                \x20 C(Caller c) : super(c);\n\
                }\n";
    assert_eq!(
        code_names(&report_two_file(main, "class Z {}\n")),
        Vec::<&str>::new()
    );
}

/// The callable class whose `call` signature disagrees: the tearoff arm
/// fails the same way the bare interface would, and the rejection reports
/// (probed: `int call(int x)` into `void Function()` reports on the
/// oracle, the argument read as the `Caller`-typed parameter's declared
/// type).
#[test]
fn callable_class_with_incompatible_call_reports() {
    let main = "class Caller {\n\
                \x20 int call(int x) => x;\n\
                }\n\
                \n\
                class B {\n\
                \x20 B(void Function() f);\n\
                }\n\
                \n\
                class C extends B {\n\
                \x20 C(Caller c) : super(c);\n\
                }\n";
    assert_eq!(
        code_names(&report_two_file(main, "class Z {}\n")),
        ["argument_type_not_assignable"]
    );
}

/// A same-file function-RHS alias formal (`typedef F = int Function(int)`)
/// rides the same face — the alias unfolds at the shape question, and the
/// `String` argument reports (the oracle reports against the written
/// alias spelling; a foreign-file alias head keeps the under-report
/// posture — the diagnosing file's bounds carry no capture for it).
#[test]
fn alias_typed_function_formal_mismatch_reports() {
    let main = "typedef F = int Function(int);\n\
                \n\
                class B {\n\
                \x20 B(F f);\n\
                }\n\
                \n\
                class C extends B {\n\
                \x20 C() : super('x');\n\
                }\n";
    assert_eq!(
        code_names(&report_two_file(main, "class Z {}\n")),
        ["argument_type_not_assignable"]
    );
}
