//! The #2417 pins: the cross-file constructor-formals projection.
//!
//! `resolve_container` returns the resolved `Container` and its owning file,
//! but the constructor-argument faces discarded the owner and cloned the
//! `ctor_formals` table verbatim. A foreign container's `formal.ty` slots
//! (`ArgListId`/`FnShapeId`/`RecordShapeId`) index *that* unit's `ArgPool`,
//! so dereferencing them through the diagnosing pool read just-past-end —
//! `model.rs:642` (`ArgListId`) and `model.rs:773` (`FnShapeId`) panicked on
//! the xml/intl corpus packages, aborting the whole package with zero
//! diagnostics where the oracle emits its rows.
//!
//! The member walk never had the gap — [`Boundary::member`] projects a
//! member's `formals` — because a constructor's table lives on `Container`,
//! not on a `MemberEntry`. The fix re-interns every `formal.ty` (and the
//! field-formal/super-formal patches that fill an untyped formal) through
//! `boundary.ty(owner, ty)` before the argument walk derefs it, on the
//! `check_super_invocation_arguments`, `check_enum_const_arguments`, and
//! `associated_super_formal_ty` paths; a same-file owner short-circuits.
//!
//! Oracle provenance: `/tmp/probe2417` (a `class B { B(void Function(String)
//! f); }` in `lib.dart`, `class C extends B { C() : super((String s) {}); }`
//! in `main.dart`) panicked `dartq` pre-fix at `model.rs:773` and reports
//! zero rows on `dart analyze` 3.13.1.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `main_source` as `main.dart` importing `lib.dart`, a two-file
/// library — the superclass (and its constructor's foreign-pool formals)
/// lives in `lib.dart`, the `: super(...)` invocation in `main.dart`.
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

/// The `FnShapeId` face (`model.rs:773`): a foreign constructor formal typed
/// `void Function(String)` interns its function shape in `lib.dart`'s pool;
/// the `: super((String s) {})` argument walk derefs it against the
/// diagnosing pool. Pre-fix this panicked; now the lambda checks against the
/// projected shape and stays silent.
#[test]
fn super_invocation_function_formal_cross_file_stays_silent() {
    let lib = "class B {\n\
               \x20 B(void Function(String) f);\n\
               }\n";
    let main = "import 'lib.dart';\n\
                class C extends B {\n\
                \x20 C() : super((String s) {});\n\
                }\n";
    assert_eq!(code_names(&report_two_file(main, lib)), Vec::<&str>::new());
}

/// The `ArgListId` face (`model.rs:642`): a foreign `List<int>` formal's
/// argument list is a `lib.dart`-pool slot; the `covariant_args` walk reads
/// it through the diagnosing pool. The const-list argument checks against
/// the projected `List<int>` and stays silent.
#[test]
fn super_invocation_generic_formal_cross_file_stays_silent() {
    let lib = "class B {\n\
               \x20 B(List<int> items);\n\
               }\n";
    let main = "import 'lib.dart';\n\
                class C extends B {\n\
                \x20 C() : super(const <int>[1]);\n\
                }\n";
    assert_eq!(code_names(&report_two_file(main, lib)), Vec::<&str>::new());
}

/// The named-selector twin (`super.named(...)`): the foreign `Map<String,
/// int>` formal projects the same way — the projected table still answers
/// the selector's formals.
#[test]
fn super_named_invocation_generic_formal_cross_file_stays_silent() {
    let lib = "class B {\n\
               \x20 B.named(Map<String, int> table);\n\
               }\n";
    let main = "import 'lib.dart';\n\
                class C extends B {\n\
                \x20 C() : super.named(<String, int>{'a': 1});\n\
                }\n";
    assert_eq!(code_names(&report_two_file(main, lib)), Vec::<&str>::new());
}

/// The projection keeps the assignability verdict, not just the panic fix:
/// a `String` argument against the projected `int` formal still reports
/// `argument_type_not_assignable` — the foreign `formal.ty` resolves to the
/// real `int`, it is not silently dropped to `dynamic`/`Unknown`.
#[test]
fn super_invocation_wrong_type_still_reports() {
    let lib = "class B {\n\
               \x20 B(int i);\n\
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
