//! The #2365 pins: the bare-identifier implicit-`call` invocation's
//! result type — `return body(inner)` over a callable class instance
//! types the send with the `call` method's return (the oracle resolves
//! `body.call` and records that member's return on the invocation node,
//! upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/function_expression_invocation_resolver.dart:141-203),
//! never with the callee's own class — the xml
//! `expressions/statement.dart:47` / `expressions/function.dart:241`
//! RETURN_OF_INVALID_TYPE over-fire root cause. A generic `call<R>(R x)`
//! solves `R` like any generic send (downward from the context type,
//! upward from the first matched argument); a non-callable callee keeps
//! the existing `invocation_of_non_function_expression` face, and the
//! dot-spelled `body.call(inner)` control locks the two spellings
//! consistent.
//!
//! The face rides the argument-check pass (the call method's formals
//! drive the count/argument checks, and its type parameters feed the
//! send's inference), so the zero-argument send's
//! `not_enough_positional_arguments` is earned by the same resolution.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module.
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

fn codes(report: &FileReport, name: &str) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| d.range.offset)
        .collect()
}

/// The xml `statement.dart:47` shape: the bare callable instance
/// invocation `body(inner)` types as `Seq` (`Expr.call`'s return), so
/// the return check asks `Seq` into `Seq` and stays silent — the callee's
/// own `Expr` class (the pre-fix face) would report.
#[test]
fn bare_callable_invocation_types_as_call_return() {
    let source = "abstract class Seq {}\n\
                  \x20abstract class Ctx {}\n\
                  \x20abstract class Expr {\n\
                  \x20  Seq call(Ctx context);\n\
                  \x20}\n\
                  \x20Seq use(Expr body, Ctx inner) {\n\
                  \x20  return body(inner);\n\
                  \x20}";
    let report = report(source);
    assert!(
        codes(&report, "return_of_invalid_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The generic-`call` substitution, matching side: `R call<R>(R x)`
/// solves `R` from the first matched argument, so `w(item)` types as
/// `Item` and the matching return stays silent.
#[test]
fn generic_call_argument_solve_satisfies_matching_return() {
    let source = "class Item {}\n\
                  \x20class Wrap {\n\
                  \x20  R call<R>(R x) => x;\n\
                  \x20}\n\
                  \x20Item take(Wrap w, Item item) {\n\
                  \x20  return w(item);\n\
                  \x20}";
    let report = report(source);
    assert!(
        codes(&report, "return_of_invalid_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The generic-`call` substitution, mismatching side: the solved `Item`
/// (not the callee's own `Wrap` class, and not the raw parameter)
/// decides the verdict — the message is the proof the substitution ran.
/// (The rig's single-file world seeds no dart:core, so the pin's types
/// are user classes.)
#[test]
fn generic_call_argument_solve_decides_mismatching_return() {
    let source = "class Item {}\n\
                  \x20class Other {}\n\
                  \x20class Wrap {\n\
                  \x20  R call<R>(R x) => x;\n\
                  \x20}\n\
                  \x20Other take(Wrap w, Item item) {\n\
                  \x20  return w(item);\n\
                  \x20}";
    let report = report(source);
    let at = codes(&report, "return_of_invalid_type");
    assert_eq!(at.len(), 1, "{:?}", report.diagnostics);
    assert!(
        report.diagnostics[0]
            .message
            .contains("A value of type 'Item' can't be returned"),
        "{:?}",
        report.diagnostics[0].message
    );
}

/// A non-callable bare callee keeps the existing face: no `call` member
/// answers, `invocation_of_non_function_expression` reports at the
/// callee, and the face's formals/result machinery stays out of the way.
#[test]
fn non_callable_bare_invocation_keeps_non_function_face() {
    let source = "class Plain {}\n\
                  \x20void use(Plain p) {\n\
                  \x20  p(1);\n\
                  \x20}";
    let report = report(source);
    let at = codes(&report, "invocation_of_non_function_expression");
    assert_eq!(at, vec![source.find("p(1)").unwrap() as u32], "{:?}", report.diagnostics);
}

/// The dot-spelled control: `body.call(inner)` resolves the same `call`
/// member through the member face and types the send with the same
/// `Seq` return — the two spellings stay consistent.
#[test]
fn explicit_call_spelling_types_the_same() {
    let source = "abstract class Seq {}\n\
                  \x20abstract class Ctx {}\n\
                  \x20abstract class Expr {\n\
                  \x20  Seq call(Ctx context);\n\
                  \x20}\n\
                  \x20Seq use(Expr body, Ctx inner) {\n\
                  \x20  return body.call(inner);\n\
                  \x20}";
    let report = report(source);
    assert!(
        codes(&report, "return_of_invalid_type").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The count face the bare formals carry: the call method's parameter
/// list counts the arguments, so the zero-argument send reports the
/// deficit (`resolveArgumentsToParameters` over the resolved `.call`).
#[test]
fn bare_callable_invocation_counts_against_call_formals() {
    let source = "abstract class Ctx {}\n\
                  \x20abstract class Expr {\n\
                  \x20  void call(Ctx context);\n\
                  \x20}\n\
                  \x20void use(Expr body) {\n\
                  \x20  body();\n\
                  \x20}";
    let report = report(source);
    let at = codes(&report, "not_enough_positional_arguments");
    assert_eq!(at.len(), 1, "{:?}", report.diagnostics);
}
