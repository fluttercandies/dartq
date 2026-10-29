//! The #2440 pins (LAW A phase A2): a closure literal passed where a
//! `Function(...)`-typed formal is expected binds its implicit parameters
//! from that formal's schema — including a schema the callee's own inference
//! parameters spell, instantiated by this call's solution.
//!
//! The census's largest remaining cluster (`/tmp/probe2437`, #2437's C2
//! family) was 339 `unchecked_use_of_nullable_value` rows, every one a member
//! read on a callback parameter: `forCorrespondingPairs(list1, list2,
//! (fragment, info) { … })` left both parameters `Object?` — the
//! unresolvable-schema recovery — so each `.member` fired the nullable gate.
//! The audit's minimal probes came back clean because a same-file generic
//! callee's parameter names resolve as the file's own `TypeParam` decls
//! (`named_type_is_type` proves them), while the corpus callee is
//! cross-file: its `T1`/`T2` leaves prove nothing at the call site. The
//! reduced ingredient, then, is exactly:
//!
//! * a decidable schema entry that names a *callee* inference parameter
//!   (`void Function(T1, T2)`), and
//! * that parameter solved from an earlier argument (`Iterable<T1>` against
//!   `List<Frag>` — the builtins `Iterable` climb), with
//! * the callee declared in another library file, so the caller's scope
//!   cannot prove the name.
//!
//! The bind-time solver ([`Engine::closure_schema_instantiate`]) mirrors the
//! send-close inference faces: explicit type arguments first, receiver
//! written instantiation as the container-parameter seed, then the upward
//! unification of the arguments already typed. Only names the callee
//! declares as inference parameters can bind; everything else keeps the
//! unresolvable-head recovery (`Object?`) — the conservative iron rule.
//!
//! Oracle provenance: every pinned shape is silent under `dart analyze`
//! (Dart 3.13.1, fvm 3.47.1) on the `/tmp/probe2440` fixtures; the shape
//! reduces the `forCorrespondingPairs` face of package_analyzer's
//! `collection.dart` with the `informative_data.dart` call shape.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `main_source` as `main.dart` importing `lib.dart` — the
/// cross-file rig the whole cluster needs (a same-file callee proves its own
/// parameter names and never reproduced the census rows).
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
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "Iterable", "void",
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

/// Single-file rig (`main.dart` with the implicit `dart:core` seed).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_main,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    builder.seed_implicit_core(
        lib_main,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "Iterable", "void",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
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

/// The nullable-use family — the cluster's whole signal.
fn unchecked_clean(names: &[&'static str]) -> bool {
    !names.contains(&"unchecked_use_of_nullable_value")
}

/// Pin (a): a concrete `Function(Frag, Info)` formal (cross-file, so the
/// schema rides the boundary projection) types both literals' parameters —
/// the member reads on `a` and `b` are silent, as the oracle's are (the
/// parameters bind `Frag`/`Info`, never `Object?`).
#[test]
fn concrete_function_formal_binds_closure_parameters() {
    let lib = "class Frag {}\n\
               class Info {}\n\
               void take(void Function(Frag a, Info b) f) {}\n";
    let main = "import 'lib.dart';\n\
                class FragImpl extends Frag { void x() {} }\n\
                class InfoImpl extends Info { int get codeOffset => 0; }\n\
                void g() {\n\
                \x20 take((a, b) { a.x(); b.codeOffset; });\n\
                }\n";
    let names = code_names(&report_two_file(main, lib));
    assert!(
        unchecked_clean(&names),
        "concrete schema parameters never read nullable: {names:?}"
    );
}

/// Pin (b), the census reduction: a cross-file generic callee whose
/// `Function(T1, T2)` formal's schema entries are the callee's own inference
/// parameters. `List<Frag>`/`List<Info>` arguments solve `T1`/`T2` through
/// the builtins `Iterable` climb, so both closure parameters bind concrete
/// types and their member reads stay silent (the corpus's 339-row shape,
/// reduced from `forCorrespondingPairs` + `informative_data.dart`'s
/// `_applyToAccessors`).
#[test]
fn generic_callee_schema_instantiates_closure_parameters() {
    let lib = "void forCorrespondingPairs<T1, T2>(\n\
               \x20 Iterable<T1> list1,\n\
               \x20 Iterable<T2> list2,\n\
               \x20 void Function(T1, T2) f,\n\
               ) {}\n";
    let main = "import 'lib.dart';\n\
                class PropertyAccessorFragmentImpl {\n\
                \x20 void setCodeRange(int codeOffset, int codeLength) {}\n\
                \x20 int get nameOffset => 0;\n\
                }\n\
                class InfoAccessor {\n\
                \x20 int get codeOffset => 0;\n\
                \x20 int get codeLength => 0;\n\
                }\n\
                void apply(\n\
                \x20 List<PropertyAccessorFragmentImpl> fragmentList,\n\
                \x20 List<InfoAccessor> infoList,\n\
                ) {\n\
                \x20 forCorrespondingPairs(fragmentList, infoList, (fragment, info) {\n\
                \x20   fragment.setCodeRange(info.codeOffset, info.codeLength);\n\
                \x20   fragment.nameOffset;\n\
                \x20 });\n\
                }\n";
    let names = code_names(&report_two_file(main, lib));
    assert!(
        unchecked_clean(&names),
        "the instantiated schema types both parameters: {names:?}"
    );
}

/// Pin (c), the member-send face the same solver serves: a cross-file
/// generic method's `void Function(T)` formal — the method's own `T`, solved
/// by the upward unification of `List<Item>` against `List<T>` (same-head
/// pairwise), and by explicit type arguments (`writeList<Item>`). Both call
/// shapes are silent, like the oracle's.
#[test]
fn generic_member_schema_instantiates_closure_parameters() {
    let lib = "class Writer {\n\
               \x20 void writeList<T>(List<T> items, void Function(T x) writeItem) {}\n\
               }\n";
    let main = "import 'lib.dart';\n\
                class Item { void write(Writer w) {} }\n\
                void g(Writer writer, List<Item> items) {\n\
                \x20 writer.writeList<Item>(items, (x) => x.write(writer));\n\
                \x20 writer.writeList(items, (x) { x.write(writer); });\n\
                }\n";
    let names = code_names(&report_two_file(main, lib));
    assert!(
        unchecked_clean(&names),
        "the member's solved T types the literal's parameter: {names:?}"
    );
}

/// Pin (d), the no-schema control: a closure with no `Function`-typed
/// context at all (`var f = (a) { a.x; };`) keeps today's verdict exactly —
/// the parameter stays `dynamic`, the member read is silent, and the
/// explicit `?.`-required shapes stay untouched. The solver must never form
/// a binding without a callee schema.
#[test]
fn no_schema_closure_keeps_dynamic_parameter() {
    let source = "class Frag { void x() {} }\n\
                  void g() {\n\
                  \x20 var f = (a) { a.x(); a.y; };\n\
                  \x20 f;\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        unchecked_clean(&names),
        "an untyped literal's parameter is dynamic, never nullable: {names:?}"
    );
    assert!(
        !names.contains(&"undefined_getter") && !names.contains(&"undefined_method"),
        "a dynamic parameter's member reads stay silent: {names:?}"
    );
}

/// Pin (e), the under-report guard: an *unprovable* schema entry that no
/// argument solves keeps the `Object?` recovery — `void Function(Und)`
/// against a call that passes nothing positionally solving `Und` still
/// fires the nullable gate on the parameter's member read, exactly as
/// before (the oracle reports the same `unchecked_use_of_nullable_value`
/// for an unresolvable schema entry, whose greatest closure is `Object?`).
#[test]
fn unsolved_schema_entry_keeps_nullable_recovery() {
    let source = "void f(void Function(Und) g) {}\n\
                  void h() {\n\
                  \x20 f((x) { x.anything; });\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "an unsolved unprovable schema entry stays Object?: {names:?}"
    );
}
