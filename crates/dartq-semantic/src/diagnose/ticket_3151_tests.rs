//! The #3151 pins: `body_might_complete_normally`'s local-function arm.
//!
//! Upstream citations:
//! - `visitFunctionDeclaration` runs `checkForBodyMayCompleteNormally`
//!   with `errorNode: node.name` for every declaration flavor — top-level,
//!   member, and LOCAL alike (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/resolver.dart:3109-3114). The
//!   `!node.isSetter` gate never applies to a local declaration (local
//!   functions cannot spell `set`), and the report anchors at the
//!   declaring name.
//! - The reachability gate is `flow.isReachable` at the body's exit —
//!   `int g() {}` falls off its `}` and reports, `int g() { return 0; }`
//!   does not (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/resolver.dart:547-550 and
//!   pkg/analyzer/lib/src/dart/resolver/exit_detector.dart's statement
//!   arms).
//! - The twin split is `typeSystem.isPotentiallyNonNullable(returnType)`:
//!   a non-nullable head reports `body_might_complete_normally`
//!   (COMPILE_TIME_ERROR), a nullable head whose `FutureOr` base is not
//!   `dynamic`/invalid/unknown/`void`/`Null` reports
//!   `body_might_complete_normally_nullable` (STATIC_WARNING), and the
//!   `dynamic`/`void`/untyped faces stay silent (upstream Dart SDK
//!   source, pkg/analyzer/lib/src/generated/resolver.dart:586-603).
//! - Generators (`sync*`/`async*`) return early — `body.isGenerator`,
//!   resolver.dart:561-563 — and an `async` body asks through
//!   `_contextTypeForImposed`'s `FutureOr<futureValueType(S)>` union
//!   (pkg/analyzer/lib/src/dart/resolver/body_inference_context.dart:257
//!   plus `TypeSystem.futureValueType`,
//!   pkg/analyzer/lib/src/dart/element/type_system.dart:471).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_3109_tests`' shape).
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

/// `(code, offset)` rows of the completion family in source order.
fn completion_rows(report: &FileReport) -> Vec<(&'static str, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "body_might_complete_normally"
                || d.code.name == "body_might_complete_normally_nullable"
        })
        .map(|d| (d.code.name, d.range.offset))
        .collect()
}

/// A declared local function whose body falls off the end reports the
/// ERROR twin at the declaring name (upstream `visitFunctionDeclaration`'s
/// `errorNode: node.name` — the name token, never the body `{`).
#[test]
fn local_function_empty_body_reports_at_name() {
    let report = report("int f() { int g() {} return g(); }");
    let rows = completion_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", completion_rows(&report));
    assert_eq!(rows[0].0, "body_might_complete_normally");
    // `g` is at UTF-16 offset 14.
    assert_eq!(rows[0].1, 14);
}

/// Two sibling local functions each report at their own name — the
/// `}`-keyed rows never bleed into one another.
#[test]
fn two_local_functions_each_report_at_own_name() {
    let report = report("int f() { int g() {} int h() {} return g() + h(); }");
    let rows = completion_rows(&report);
    assert_eq!(
        rows,
        vec![
            ("body_might_complete_normally", 14),
            ("body_might_complete_normally", 25),
        ],
        "{:?}",
        completion_rows(&report)
    );
}

/// A local function inside a method reports identically — the enclosing
/// shape is irrelevant to the declaration's own walk.
#[test]
fn local_function_inside_method_reports() {
    let report = report("class A { int m() { int g() {} return g(); } }");
    assert_eq!(
        completion_rows(&report),
        vec![("body_might_complete_normally", 24)]
    );
}

/// A local function inside a constructor body reports identically.
#[test]
fn local_function_inside_constructor_reports() {
    let report = report("class A { A() { int g() {} g(); } }");
    assert_eq!(
        completion_rows(&report),
        vec![("body_might_complete_normally", 20)]
    );
}

/// A local function inside a function literal reports identically.
#[test]
fn local_function_inside_closure_reports() {
    let report = report("var f = () { int g() {} return g(); };");
    assert_eq!(
        completion_rows(&report),
        vec![("body_might_complete_normally", 17)]
    );
}

/// A local function inside a switch case reports identically.
#[test]
fn local_function_inside_switch_case_reports() {
    let report =
        report("int f(int x) { switch (x) { case 1: int g() {} return g(); default: return 0; } }");
    assert_eq!(
        completion_rows(&report),
        vec![("body_might_complete_normally", 40)]
    );
}

/// A nested local function reports at its own name while the enclosing
/// local function's missing return reports at its name — both rows
/// present, each at its declaring name (s48's two-row shape).
#[test]
fn nested_local_function_reports_both_names() {
    let report = report("int f() { int g() {} }");
    assert_eq!(
        completion_rows(&report),
        vec![
            ("body_might_complete_normally", 14),
            ("body_might_complete_normally", 4),
        ]
    );
}

/// A deeper nesting chain reports only the frames that actually fall off
/// the end: `h` falls off, `g` returns, `f` returns.
#[test]
fn deep_nested_local_function_reports_only_fallen_bodies() {
    let report = report("int f() { int g() { int h() {} return h(); } return g(); }");
    assert_eq!(
        completion_rows(&report),
        vec![("body_might_complete_normally", 24)]
    );
}

/// A local function whose body returns reports nothing.
#[test]
fn local_function_with_return_stays_silent() {
    let report = report("int f() { int g() { return 0; } return g(); }");
    assert_eq!(completion_rows(&report), Vec::new());
}

/// A local function whose body throws reports nothing.
#[test]
fn local_function_that_throws_stays_silent() {
    let report = report("int f() { int g() { throw 0; } return g(); }");
    assert_eq!(completion_rows(&report), Vec::new());
}

/// An arrow-bodied local function reports nothing — the official
/// `BlockFunctionBody` gate skips `=>` bodies entirely.
#[test]
fn local_function_arrow_body_stays_silent() {
    let report = report("int f() { int g() => 0; return g(); }");
    assert_eq!(completion_rows(&report), Vec::new());
}

/// A `void` local function reports nothing — the return-type face is one
/// of the official silent tops.
#[test]
fn local_function_void_stays_silent() {
    let report = report("void f() { void g() {} g(); }");
    assert_eq!(completion_rows(&report), Vec::new());
}

/// An untyped local function reports nothing — the absent return type is
/// the oracle's `dynamic` face (`contextType` null/dynamic, no verdict).
#[test]
fn local_function_untyped_stays_silent() {
    let report = report("int f() { g() {} return g(); }");
    assert_eq!(completion_rows(&report), Vec::new());
}

/// A nullable local-function return reports the NULLABLE twin at the
/// declaring name.
#[test]
fn local_function_nullable_return_reports_nullable_twin() {
    let report = report("int f() { int? g() {} return g() ?? 0; }");
    assert_eq!(
        completion_rows(&report),
        vec![("body_might_complete_normally_nullable", 15)]
    );
}

/// A `Never`-returning local function reports the ERROR twin — `Never`
/// is potentially non-nullable.
#[test]
fn local_function_never_return_reports_error() {
    let report = report("int f() { Never g() {} return g(); }");
    assert_eq!(
        completion_rows(&report),
        vec![("body_might_complete_normally", 16)]
    );
}

/// A `sync*` local function reports nothing — `body.isGenerator` returns
/// early upstream (resolver.dart:561-563).
#[test]
fn local_function_sync_generator_stays_silent() {
    let report = report(
        "class Iterable<T> {}\n\
         int f() { Iterable<int> g() sync* {} return 0; }",
    );
    assert_eq!(completion_rows(&report), Vec::new());
}
