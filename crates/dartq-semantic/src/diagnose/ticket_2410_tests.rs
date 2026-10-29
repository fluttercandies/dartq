//! The #2410 pins: the typedef RHS seam at the chase site — the four RHS
//! shapes that previously landed opaque now resolve, each pinned against the
//! oracle verdict probed on the `/tmp/probe2410` fixtures (dart analyze,
//! Dart 3.13.1 via fvm 3.47.1).
//!
//! The resolved-type face (the mounted [`LibraryTypes`] snapshot): the
//! alias's RHS flattens into the snapshot's lattice at registration — an
//! argument-bearing `Named`, a function shape, a record shape, and a
//! parameterized alias's captured RHS with its parameter leaves — so the
//! chase answers through the real shape instead of the opaque face. Each
//! equivalence pin pairs the alias spelling with the direct spelling: the
//! oracle flags the second test `unnecessary_type_check` (always true) and
//! deads its else branch, and the resolved face answers identically only
//! when the alias maps to the very type its RHS spells.
//!
//! The substitution face (the diagnosing fold): a parameterized use with
//! written arguments rewrites to the captured RHS with the alias's own
//! parameters substituted — the two-file `Predicate<String> pred; pred(42)`
//! pin reports `argument_type_not_assignable` at the argument, the flipped
//! #2413 red pin (the same verdict the oracle emits for the direct
//! `bool Function(String)` spelling).

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the chase seam the alias shapes answer through
/// (the `ticket_dead_family_tests` rig mirrored, with the core type
/// spellings seeded beside — the fixtures name `List`/`Map` directly).
fn report_ops(source: &str) -> FileReport {
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
    builder.unit(lib, file);
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
            "Object", "String", "int", "bool", "double", "num", "List", "Map",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
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
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// Diagnoses `main_source` as `main.dart` importing `lib.dart` (the
/// `ticket_2413_tests` two-file rig): the alias lives in `lib.dart`, the
/// use sites in `main.dart`.
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
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The chase evidence row: the inner `x is <direct spelling>` inside the
/// aliased test factors to `Never` (the mapped RHS answers the real
/// subtype edge), so the losing else branch reports `dead_code` anchored
/// at its first statement — the oracle verdict the fixture carries
/// (probed: /tmp/probe2410/p2b, p3b, p4b, p5 — the oracle pairs the else
/// `dead_code` with `unnecessary_type_check` on the condition, a face the
/// operand-static-type walk answers; the chase-reachable half is the
/// dead branch). Without the shape mapping the alias reads opaque, the
/// factor keeps the promoted type, and no branch deads — the pin
/// discriminates exactly the chase.
fn assert_else_dead(report: &FileReport, source: &str) {
    let else_start = source.find("else").unwrap() as u32;
    let dead = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    assert!(
        dead.iter().any(|&offset| offset >= else_start),
        "the else arm of the always-true pair is dead: {:?}",
        code_names(report)
    );
}

/// Shape 1 — argument-bearing `Named` RHS: `typedef A = List<int>` maps to
/// `List<int>` at the chase, so the direct spelling inside `x is A` reads
/// always-true and its else deads (the /tmp/probe2410/p2b verdicts: the
/// oracle pairs `unnecessary_type_check` on the condition with
/// `dead_code` on the else body).
#[test]
fn argument_bearing_named_rhs_resolves_at_the_chase() {
    let source = "typedef A = List<int>;\n\
                  void f(Object x) {\n\
                  \x20 if (x is A) {\n\
                  \x20   if (x is List<int>) {\n\
                  \x20     x;\n\
                  \x20   } else {\n\
                  \x20     'else';\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n";
    assert_else_dead(&report_ops(source), source);
}

/// Shape 2 — function-shaped RHS: `typedef F = int Function(String)` maps
/// to the interned signature at the chase (the /tmp/probe2410/p3b
/// verdicts: the same else-dead pair).
#[test]
fn function_shaped_rhs_resolves_at_the_chase() {
    let source = "typedef F = int Function(String);\n\
                  void f(Object x) {\n\
                  \x20 if (x is F) {\n\
                  \x20   if (x is int Function(String)) {\n\
                  \x20     x;\n\
                  \x20   } else {\n\
                  \x20     'else';\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n";
    assert_else_dead(&report_ops(source), source);
}

/// Shape 3 — record-shaped RHS: `typedef R = (int, String)` maps to the
/// interned record shape at the chase (the /tmp/probe2410/p4b verdicts:
/// the same else-dead pair).
#[test]
fn record_shaped_rhs_resolves_at_the_chase() {
    let source = "typedef R = (int, String);\n\
                  void f(Object x) {\n\
                  \x20 if (x is R) {\n\
                  \x20   if (x is (int, String)) {\n\
                  \x20     x;\n\
                  \x20   } else {\n\
                  \x20     'else';\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n";
    assert_else_dead(&report_ops(source), source);
}

/// Shape 4 — parameterized alias: the captured RHS substitutes the
/// written arguments at the chase (`P<int>` reads `Map<String, int>`),
/// so the direct spelling inside `x is P<int>` reads always-true (the
/// /tmp/probe2410/p5 verdicts: the oracle pairs
/// `unnecessary_type_check` on the condition — the substitution
/// evidence it emits itself — with `dead_code` on the else body).
#[test]
fn parameterized_alias_substitutes_at_the_chase() {
    let source = "typedef P<T> = Map<String, T>;\n\
                  void g(Object x) {\n\
                  \x20 if (x is P<int>) {\n\
                  \x20   if (x is Map<String, int>) {\n\
                  \x20     x;\n\
                  \x20   } else {\n\
                  \x20     'else';\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n";
    assert_else_dead(&report_ops(source), source);
}

/// The substitution face end-to-end (the flipped #2413 red pin's face, probed
/// on /tmp/probe2410/p1_main.dart + typedefs.dart): a foreign parameterized
/// alias use folds to the aliased signature at the formal's bind, and
/// `pred(42)` counts against the instantiated formals —
/// `argument_type_not_assignable` at the `42`, exactly the oracle's row.
#[test]
fn foreign_parameterized_alias_invocation_argument_is_checked() {
    let main = "import 'lib.dart';\n\
                bool run(Predicate<String> pred) {\n\
                \x20 return pred(42);\n\
                }\n";
    let lib = "typedef Callback<T, R> = R Function(T value);\n\
               typedef Predicate<T> = Callback<T, bool>;\n";
    let report = report_two_file(main, lib);
    let names = code_names(&report);
    assert!(
        names.contains(&"argument_type_not_assignable"),
        "the aliased formals substitute: {names:?}"
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .expect("argument row");
    assert_eq!(row.range.length, 2, "the span covers the `42` argument");
}

/// A resolved alias keeps its benign uses silent — the shapes must not
/// leak verdicts the oracle does not emit (the oracle's own posture on a
/// lone narrowing is-check is zero rows, probed on /tmp/probe2410/p2c).
#[test]
fn resolved_alias_keeps_a_benign_use_silent() {
    let source = "typedef A = List<int>;\n\
                  typedef P<T> = Map<String, T>;\n\
                  void f(Object x) {\n\
                  \x20 if (x is int) {\n\
                  \x20   if (x is A) {\n\
                  \x20     x;\n\
                  \x20   }\n\
                  \x20 }\n\
                  \x20 if (x is P<int>) {\n\
                  \x20   x;\n\
                  \x20 }\n\
                  }\n";
    let report = report_ops(source);
    assert!(code_names(&report).is_empty(), "{:?}", code_names(&report));
}
