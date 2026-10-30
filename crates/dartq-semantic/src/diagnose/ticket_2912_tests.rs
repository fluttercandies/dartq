//! The #2912 null-safety assignability pins: the whole-expression anchors
//! and operand faces the staged-check teardown gained.
//!
//! 1. The staged row's emission span is the whole `Expression` node —
//!    `await e`, `new C()`, a parenthesized or unary operand, `x as T`,
//!    postfix `!` — not the operand's own read row (upstream Dart SDK
//!    source, pkg/analyzer/lib/src/dart/resolver/
//!    assignment_expression_resolver.dart:167-178 `.at(right)`,
//!    return_type_verifier.dart:150-206 `.at(expression)`).
//! 2. `extension E<T> on T` applicability: the bare `on` type-parameter
//!    leaf binds the receiver through `constrainArgument`'s absorption
//!    rule (extension_member_resolver.dart:430-455), so a nullable
//!    receiver still resolves the member — no `unchecked_use_of_nullable_
//!    value`.
//! 3. `return x as T` asks the CAST's resolved target type, not the
//!    operand's parse-time face — `return null as int` inside a `void`
//!    host reports the `int` face (`filter_staged`'s `cast_rewrites`
//!    substitution).
//! 4. `x += await e` types the augmented-operator send on the awaited
//!    value (`flatten(Future<int>)` = `int`), not the `Future` face.
//! 5. A function-typed return annotation (`T Function<T>(T, T) get foo`)
//!    captures in the executable's head region — before any body event —
//!    instead of degrading to `dynamic Function()`.
//!
//! Oracle provenance: `StaticTypeWarningCodeTest.test_await_simple`
//! (`dart analyze`, Dart 3.13) for the `await` anchor; the co19
//! `Expect.expectStaticType` extension shape for the `on T` binding;
//! `ReturnOfInvalidTypeTest.test_function_sync_block_int__to_void` for
//! the void-host verdict text.

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, fold_unit, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
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

fn rows_named<'a>(report: &'a FileReport, code: &str) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort();
    rows
}

// —— shape 1: the whole-expression anchor ——

/// `String a = await fi();` — the `await fi()` invocation anchors at the
/// `await` keyword through `)` (the oracle's `4:14 len 10` row), not the
/// callee's own read (`fi`, `4:20 len 4` — the pre-fix extra).
#[test]
fn await_operand_anchors_at_the_await_expression() {
    let report = report(
        "Future<int> fi() => Future.value(0);\n\
         f() async {\n\
         \x20 String a = await fi();\n\
         }\n",
    );
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one invalid_assignment row: {rows:?}");
    let (offset, length, message) = &rows[0];
    assert_eq!(
        message,
        "A value of type 'int' can't be assigned to a variable of \
         type 'String'.",
        "the message face: {rows:?}"
    );
    // The span itself is the contract: `await fi()` starts on the keyword.
    assert_eq!(*length, 10, "await fi() is ten units: {rows:?}");
    assert_eq!(
        *offset,
        "Future<int> fi() => Future.value(0);\nf() async {\n  String a = ".len() as u32,
        "the anchor sits on the `await` keyword: {rows:?}"
    );
}

/// `x = (e)`: the parenthesized operand's node includes the parens —
/// `int a = (fi());` anchors at `(` through `)`.
#[test]
fn parenthesized_operand_anchors_at_the_parens() {
    let report = report(
        "int fi() => 0;\n\
         void f() {\n\
         \x20 String a = (fi());\n\
         }\n",
    );
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one invalid_assignment row: {rows:?}");
    let (offset, length, _) = &rows[0];
    // `(fi())` — from `(` through the outer `)`.
    assert_eq!(
        *offset,
        "int fi() => 0;\nvoid f() {\n  String a = ".len() as u32,
        "the anchor sits on `(`: {rows:?}"
    );
    assert_eq!(*length, 6, "(fi()) is six units: {rows:?}");
}

/// `x = e as T`: the as-expression's node runs through the tested type —
/// `String a = null as int` anchors at `null` through `int`.
#[test]
fn as_expression_anchors_through_the_tested_type() {
    let report = report(
        "void f() {\n\
         \x20 String a = null as int;\n\
         }\n",
    );
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one invalid_assignment row: {rows:?}");
    let (offset, length, _) = &rows[0];
    assert_eq!(
        *offset,
        "void f() {\n  String a = ".len() as u32,
        "the anchor sits on `null`: {rows:?}"
    );
    assert_eq!(*length, 11, "null as int is eleven units: {rows:?}");
}

/// `x = !e`: the unary prefix rides the operand's span — `String a =
/// !flag;` anchors at `!` through the operand end.
#[test]
fn unary_operand_anchors_at_the_operator() {
    let report = report(
        "bool flag = true;\n\
         void f() {\n\
         \x20 String a = !flag;\n\
         }\n",
    );
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "one invalid_assignment row: {rows:?}");
    let (offset, length, _) = &rows[0];
    assert_eq!(
        *offset,
        "bool flag = true;\nvoid f() {\n  String a = ".len() as u32,
        "the anchor sits on `!`: {rows:?}"
    );
    assert_eq!(*length, 5, "!flag is five units: {rows:?}");
}

// —— shape 2: `extension E<T> on T` applicability ——

/// `extension StaticType<T> on T` over a nullable receiver: `T` binds the
/// receiver's non-null face, the extension applies, and the member
/// resolves — the oracle stays silent (the `expectStaticType` extras were
/// the applicability gate failing on `is_subtype(receiver, T)`).
#[test]
fn bare_on_type_parameter_binds_the_receiver() {
    let report = report(
        "extension StaticType<T> on T {\n\
         \x20 T tag<R>(R r) => r as T;\n\
         }\n\
         void f(int? x) {\n\
         \x20 x.tag<int>(0);\n\
         }\n",
    );
    let rows = rows_named(&report, "unchecked_use_of_nullable_value");
    assert!(
        rows.is_empty(),
        "the `on T` extension applies to the nullable receiver: {rows:?}"
    );
}

/// A nullable `on T` leaf (`extension E<T> on T?`) binds the receiver
/// verbatim and applies the same way.
#[test]
fn nullable_on_type_parameter_binds_the_receiver() {
    let report = report(
        "extension E<T> on T? {\n\
         \x20 int m() => 0;\n\
         }\n\
         void f(int? x) {\n\
         \x20 x.m();\n\
         }\n",
    );
    let rows = rows_named(&report, "unchecked_use_of_nullable_value");
    assert!(
        rows.is_empty(),
        "the `on T?` extension applies to the nullable receiver: {rows:?}"
    );
}

// —— shape 3: `return x as T` reads the cast's target ——

/// `return null as int` inside a `void` host: the expression's static
/// type is `int` (the cast's resolved target), so the verdict is the
/// `int → void` negative — not `Null → void`'s silent pass.
#[test]
fn return_as_cast_reads_the_target_type() {
    let report = report(
        "void f() {\n\
         \x20 return null as int;\n\
         }\n",
    );
    let rows = rows_named(&report, "return_of_invalid_type");
    assert_eq!(rows.len(), 1, "the cast target reports: {rows:?}");
    let (_, _, message) = &rows[0];
    assert_eq!(
        message,
        "A value of type 'int' can't be returned from the function 'f' \
         because it has a return type of 'void'.",
        "the int face reports, never Null: {rows:?}"
    );
}

// —— shape 4: `x += await e` reads the awaited type ——

/// `i += await fi()` over `Future<int>`: the augmented `+` send reads the
/// flattened `int`, so `int + int` answers `int` and the assignment is
/// clean — the `num`-face extra came from reading `Future<int>`'s
/// receiver face.
#[test]
fn compound_assignment_await_reads_the_flattened_type() {
    let report = report(
        "Future<int> fi() async => 0;\n\
         void f() async {\n\
         \x20 int i = 0;\n\
         \x20 i += await fi();\n\
         }\n",
    );
    let rows = rows_named(&report, "invalid_assignment");
    assert!(
        rows.is_empty(),
        "the flattened operand refines to int: {rows:?}"
    );
}

// —— shape 5: the head-region capture ——

/// `T Function<T>(T, T) get foo`: the generic function type in the
/// getter's return-annotation slot (the executable's head region —
/// between the frame's push and any body event) captures a real
/// signature, so `return null` reports against `T Function<T>(T, T)`,
/// never the shapeless `dynamic Function()`.
#[test]
fn function_typed_return_annotation_captures_in_the_head_region() {
    let report = report(
        "T Function<T>(T, T) get foo {\n\
         \x20 return null;\n\
         }\n",
    );
    let rows = rows_named(&report, "return_of_invalid_type");
    assert_eq!(rows.len(), 1, "one return_of_invalid_type row: {rows:?}");
    let (_, _, message) = &rows[0];
    assert!(
        message.contains("T Function<T>(T, T)"),
        "the captured signature displays, not dynamic Function(): {rows:?}"
    );
}
