//! The #2432 pins: invocation-level generic type-argument inference (LAW A
//! phase A1) — the name-keyed binding materialization.
//!
//! `Ty` carries no type-variable variant; a call site's type parameters
//! keep their `Ty::Named` spelling and the solved bindings materialize as
//! data at structurally derivable points: the callee's own parameters come
//! from `exec_params` (the `pending_exec_name`/`exec_bounds` pair for
//! top-level functions, the member face's `type_param_names` for sends),
//! receiver-seeded bindings come from a `Receiver::Typed`/`This` whose
//! written head arguments zip onto the declaring container's parameters,
//! and the upward face unifies each argument against its formal through
//! [`Engine::unify_upward`] — bare non-nullable parameter leaves, matched
//! `Named` heads (with the `Iterable`↔`List`/`Set` builtin edge), function
//! and record positional pairs. Conflicting candidates poison the
//! parameter (the oracle's LUB answer is deliberately not modeled — silent
//! beats the invented row the first-wins binding produced, probe
//! `sink(1, "two")`), and a prior-filled slot (explicit argument, downward
//! seed, receiver seed) is never touched. The solved map substitutes
//! through [`Engine::subst_params`] into the declared return face, so the
//! call's result carries the instantiated type.
//!
//! The alias-unfold seam folded in with it: a prefixed continuation head
//! (`shared.X<A,B>`) and a prefixed creation (`fb.ListReader<idl.X>(...)`)
//! counted their group's arguments against the bare-scope homonym (a local
//! typedef of the same spelling, or the `_constructor` face's class/ctor
//! mis-attribution). [`Engine::prefixed_head`] recovers the written prefix
//! textually — the armed-head tuple stays prefix-free — and
//! [`Engine::declared_type_params_prefixed`] counts the element the prefix
//! scope actually names.
//!
//! Oracle provenance: /tmp/probe2432 — `dart analyze` 3.13.1 is silent on
//! every pin's positive case; the pre-fix dartq reported
//! `argument_type_not_assignable` on `sink(1, "two")` and the
//! `WRONG_NUMBER_OF_TYPE_ARGUMENTS*` family on the prefixed sites.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library — the same
/// minimal rig `ticket_2426_tests` carries.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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

/// Diagnoses `main_source` importing `lib_source` through `prefix` — the
/// prefixed-arity seam's two-file rig.
fn report_prefixed(main_source: &str, lib_source: &str, prefix: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_lib = files.intern(Path::new("lib.dart"));
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
            prefix: Some(strings.intern(prefix)),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
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

fn has_code(report: &FileReport, code: &str) -> bool {
    report.diagnostics.iter().any(|d| d.code.name == code)
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// Implicit type arguments at a member send: `readEnum<E>(List<E>)` on a
/// plain receiver solves `E := Tag` from the argument's head and
/// substitutes the return face — `tag` reads `Tag`, so a switch whose
/// cases cover the enum stays silent (the pre-inference `E`-spelled
/// result left the pattern checks without a verdict), and a `String`
/// assign against the instantiated result reports the oracle's
/// `invalid_assignment` — the row an unsolved `E`/`Unknown` result could
/// never produce.
#[test]
fn implicit_args_member_send_solves_and_types_the_result() {
    // The rig carries no dart:core — `List`/`String`/`int` are declared as
    // minimal local stubs so their spellings resolve (a bare `undefined_`
    // row would poison the literal's computed `Ty` the inference reads).
    let source = "class List<E> { E get first => throw 0; } \
                  class String {} \
                  enum Tag { a, b } \
                  class Reader { E readEnum<E>(List<E> values) => values.first; } \
                  void f(Reader r) { \
                  var tag = r.readEnum(<Tag>[Tag.a]); \
                  switch (tag) { case Tag.a: break; case Tag.b: break; } \
                  }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "constant_pattern_never_matches_value_type"),
        "{:?}",
        code_names(&rep)
    );
    assert!(!has_code(&rep, "dead_code"), "{:?}", code_names(&rep));
    assert!(
        !has_code(&rep, "non_exhaustive_switch"),
        "{:?}",
        code_names(&rep)
    );

    // The instantiated result type itself: `tag` reads `Tag` off the
    // solved `E` binding, so a `Sink` assign reports the oracle's
    // `invalid_assignment` — an unsolved `E`/`Unknown` result stays silent.
    // (`Sink`, not `String`: the primitive spellings take the SDK table's
    // open verdict — an unindexed edge — so only a user-declared target
    // yields the decidable refusal.)
    let source = "class List<E> { E get first => throw 0; } \
                  class Sink {} \
                  enum Tag { a, b } \
                  class Reader { E readEnum<E>(List<E> values) => values.first; } \
                  void f(Reader r) { \
                  var tag = r.readEnum(<Tag>[Tag.a]); \
                  Sink s = tag; \
                  }";
    let rep = report(source);
    assert!(
        has_code(&rep, "invalid_assignment"),
        "{:?}",
        code_names(&rep)
    );
}

/// A top-level generic function's upward unification binds into a
/// compound return face: `wrap`'s `T := int` on the argument substitutes
/// `List<T>` to `List<int>` — the `var`-inferred `ys` then reads the
/// instantiated type, so assigning it to `String` reports
/// `invalid_assignment` exactly as the oracle does (an `E`-spelled or
/// `Unknown` result would stay silent).
#[test]
fn implicit_args_top_level_function_solves_into_the_compound_return() {
    let source = "class List<E> { E get first => throw 0; } \
                  class Sink {} \
                  class int {} \
                  List<T> wrap<T>(T x) => <T>[x]; \
                  void f() { var ys = wrap(1); Sink s = ys; }";
    let rep = report(source);
    assert!(
        has_code(&rep, "invalid_assignment"),
        "{:?}",
        code_names(&rep)
    );
}

/// A partially solved invocation — `second`'s `S` materializes from the
/// argument while `T` binds nothing — stays silent: the unsolved `T`
/// return reads as its parameter spelling, which the unknown-adjacent
/// assignability face suppresses (the oracle answers `dynamic`; the
/// under-report direction matches it). The explicit slot's precedence
/// pin: `fill<int>(true, 3)` binds `T := int` before the upward face runs,
/// so the `bool` argument reports against the solved formal.
#[test]
fn partial_args_unsolved_slot_stays_silent_and_explicit_wins() {
    let source = "class int {} \
                  T second<T, S>(S s) => throw 0; \
                  void f() { var x = second(1); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "invalid_assignment"),
        "{:?}",
        code_names(&rep)
    );

    let source = "class List<E> {} \
                  class int {} \
                  class bool {} \
                  List<T> fill<T>(T item, int count) => <T>[]; \
                  void f() { var l = fill<int>(true, 3); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// The ambiguous-site control: two arguments bind `T` to different heads
/// (`int` and `String`). The oracle answers its LUB (`Object`) and stays
/// silent; the pre-fix first-wins binding solved `T := int` and invented
/// `argument_type_not_assignable` on `"two"`. Conflicting candidates now
/// poison `T` — the unsolved formal suppresses the check.
#[test]
fn ambiguous_arguments_stay_unsolved_and_silent() {
    let source = "class int {} \
                  class String {} \
                  T sink<T>(T a, T b) => a; \
                  void f() { var s = sink(1, \"two\"); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// The alias-unfold seam: a prefixed continuation head counts its group
/// against the prefix scope's element, never a bare-scope homonym — the
/// analyzer corpus's `shared.GeneratedTypeConstraint<…>` counted against
/// the typedef being declared (0 params) and invented
/// `wrong_number_of_type_arguments`. A genuine mismatch through the same
/// prefix still reports.
#[test]
fn prefixed_continuation_head_counts_against_the_prefix_scope() {
    let lib = "typedef GeneratedTypeConstraint<A> = A Function();\n";
    let main = "import 'lib.dart' as shared;\n\
                typedef GeneratedTypeConstraint = shared.GeneratedTypeConstraint<int>;\n\
                GeneratedTypeConstraint? g;\n";
    let rep = report_prefixed(main, lib, "shared");
    assert!(
        !has_code(&rep, "wrong_number_of_type_arguments"),
        "{:?}",
        code_names(&rep)
    );

    let main = "import 'lib.dart' as shared;\n\
                shared.GeneratedTypeConstraint<int, String>? g;\n";
    let rep = report_prefixed(main, lib, "shared");
    assert!(
        has_code(&rep, "wrong_number_of_type_arguments"),
        "{:?}",
        code_names(&rep)
    );
}

/// The prefixed creation seam: `fb.ListReader<idl.X>(...)` latched the
/// prefix as the class and the class as the selector, reporting
/// `wrong_number_of_type_arguments_constructor` beside the oracle's
/// silence. A mismatched group through the same spelling keeps the base
/// code.
#[test]
fn prefixed_creation_counts_the_class_not_the_selector() {
    // `int`/`String` spelled bare in `main` resolve in main's own scope
    // (the import is prefix-only): they are declared locally so the
    // type-argument group's elements resolve — an unresolved argument would
    // report `non_type_as_type_argument` instead of exercising the arity
    // path under pin. The `const` context carries the constructor-reference
    // events (`creation_ref`); a bare `p.C<T>(args)` expression parses as a
    // prefixed member send and never arms the creation head.
    let lib = "class ListReader<T> { const ListReader(int v); }\n";
    let main = "import 'lib.dart' as fb;\n\
                class int {} class String {}\n\
                const r = fb.ListReader<int>(0);\n";
    let rep = report_prefixed(main, lib, "fb");
    assert!(
        !has_code(&rep, "wrong_number_of_type_arguments_constructor"),
        "{:?}",
        code_names(&rep)
    );
    assert!(
        !has_code(&rep, "wrong_number_of_type_arguments"),
        "{:?}",
        code_names(&rep)
    );

    let main = "import 'lib.dart' as fb;\n\
                class int {} class String {}\n\
                const r = fb.ListReader<int, String>(0);\n";
    let rep = report_prefixed(main, lib, "fb");
    assert!(
        has_code(&rep, "wrong_number_of_type_arguments"),
        "{:?}",
        code_names(&rep)
    );
}
