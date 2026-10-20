//! The #2419 pins: each depth-0 formal-parameter list opens a fresh pool.
//!
//! `diagnostic.g.dart` is a generated table of `const … Function({required
//! String name})` entries — sibling top-level generalized-function-type
//! (GFT) literals reusing the same named-formal spellings across
//! independent lists. `begin_formal_parameters` gated the
//! `positional_formals` / `named_formals` clear behind
//! `formal_depth == 0 && kind != GeneralizedFunctionType`, so a depth-0
//! GFT group (a top-level variable's type argument, a typedef RHS) opened
//! without clearing and its named formals landed in the accumulated vec;
//! the duplicate check in `end_formal_parameter` then compared across
//! sibling GFT lists — 553 `duplicate_definition` rows in the analyzer
//! corpus, effectively file-wide.
//!
//! The fix clears at every depth-0 begin; the operator return-type latch
//! and the `executable_header_done` mark stay behind the kind gate (a
//! depth-0 GFT annotation precedes the executable's own name and type
//! parameters, and `attach_type_param_count` only lands while the header
//! is open). The close-side guard keeps its kind clause: a depth-0 GFT
//! group must not `attach_formals` first-wins over the real parameter
//! list that follows (`int Function() topFn(int x)`).
//!
//! Oracle provenance: `/tmp/probe2419/pins/c.dart` against `dart analyze`
//! 3.13.1 reports exactly two rows — the within-list duplicates
//! (`Function({int d, int d})` and `g2({required int m2, required int
//! m2})`) — and stays silent on every sibling-GFT shape pinned silent
//! below.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of an import-free library — the
/// single-file rig mirrored from the sibling ticket suites.
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
            "Object", "String", "int", "bool", "double", "num", "List", "Map",
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

/// The corpus exemplar (`analyzer/lib/src/diagnostic/diagnostic.g.dart`):
/// sibling top-level GFT const variables reusing `name` across
/// independent lists. Pre-fix the third entry's `name` collided with the
/// first's and reported `duplicate_definition`; the oracle stays silent.
#[test]
fn sibling_top_level_gft_formals_stay_independent() {
    let source = "void handler({String? name, String? memberName}) => throw '';\n\
                  \x20 const void Function({required String name}) p1 = handler;\n\
                  \x20 const void Function({required String memberName}) p2 = handler;\n\
                  \x20 const void Function({required String name}) p3 = handler;\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// Typedef RHS twins (`typedef F = void Function({int e1}); typedef G =
/// void Function({int e1});`): two independent lists at library scope —
/// pre-fix `G`'s `e1` collided with `F`'s.
#[test]
fn typedef_rhs_gft_formals_stay_independent() {
    let source = "typedef F = void Function({int e1});\n\
                  typedef G = void Function({int e1});\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// A nested GFT's named formals ride the shape columns (depth ≥ 2 leaves
/// `end_formal_parameter` before the pool) and never enter the sibling
/// comparison: two siblings sharing `int Function({int? n})` stay silent.
/// All formals nullable — a non-nullable optional named formal on the
/// executable itself reports `missing_default_value_for_parameter` in
/// both engines (oracle `/tmp/probe2419/pins/d.dart` 1:42) and would
/// pollute this pin's exact set.
#[test]
fn nested_gft_formals_never_enter_sibling_pool() {
    let source = "void handler(int Function({int? n}), {int? k}) => throw '';\n\
                  \x20 const void Function(int Function({int? n}), {int? k}) n1 = handler;\n\
                  \x20 const void Function(int Function({int? n}), {int? k}) n2 = handler;\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// A GFT return type's formals and the executable's own formals are
/// different pools: `void Function({required int s}) ph({required int s})`
/// stays silent (pre-fix the kind gate happened to clear at the
/// executable's begin — the unconditional depth-0 clear preserves that).
#[test]
fn gft_return_formals_do_not_collide_with_executable_formals() {
    let source = "void Function({required int s}) ph({required int s}) => throw '';\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The header latch must stay armed through a depth-0 GFT annotation:
/// `void Function() pt<T>() => throw '';` parses `<T>` after the
/// annotation, and `attach_type_param_count` only lands while
/// `executable_header_done` is still clear. Same for the member twin.
#[test]
fn generic_function_with_gft_return_keeps_type_parameters() {
    let source = "void Function() pt<T>() => throw '';\n\
                  class Pc {\n\
                  \x20 void Function() t<T>() => throw '';\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The guard-comment shape (`int Function() pq(int x)`): the annotation's
/// `()` must not attach first-wins over the real parameter list, and the
/// depth-0 clear between the two groups must not lose `x`.
#[test]
fn gft_return_annotation_keeps_executable_formals() {
    let source = "int Function() pq(int x) => _mk;\n\
                  int _mk() => 0;\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// Member pools unchanged: a GFT-typed field and a method sharing the
/// named spelling `m` stay silent — the method's own list is cleared by
/// its own depth-0 begin.
#[test]
fn member_gft_field_and_method_formals_stay_independent() {
    let source = "class Pc {\n\
                  \x20 void Function({int m})? f;\n\
                  \x20 void g({required int m}) {}\n\
                  }\n";
    assert_eq!(code_names(&report(source)), Vec::<&str>::new());
}

/// The true positive the oracle keeps (`checkParameters`, within one
/// list): a GFT literal repeating a named formal reports
/// `duplicate_definition`, and a method's own list does the same.
#[test]
fn within_list_named_duplicates_still_report() {
    let source = "void Function({int d, int d})? pd;\n\
                  void g2({required int m2, required int m2}) {}\n";
    assert_eq!(
        code_names(&report(source)),
        ["duplicate_definition", "duplicate_definition"]
    );
}
