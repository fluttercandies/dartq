//! The #2921 `argument_type_not_assignable` pins: the assignability
//! faces the staged-check teardown and the function-subtype machinery
//! decide.
//!
//! 1. The bottom type's acceptance: `Never` is a subtype of every
//!    target (`subtype.dart`'s Left Bottom), so a `Never`-typed
//!    argument or operand never fires the code — `takesNull(never)`,
//!    `a + never`, `nil = never`.
//! 2. The function-subtype rules (`_isFunctionSubtypeOf`,
//!    subtype.dart:86-171): arity (fewer source positionals than the
//!    target asks), the required-vs-optional positional split (a
//!    required source formal cannot serve an optional target slot it
//!    never sees), formal contravariance, return covariance, and the
//!    named coverage — each decided on the two written signatures.
//! 3. The implicit-call supertype face (`isAssignableTo`'s
//!    callable-class acceptance, type_system.dart:875-884 +
//!    `acceptsFunctionType`, type_system.dart:121-126): a non-nullable
//!    interface type whose `call` member is signature-compatible assigns
//!    into a function-typed formal — and the bare `Function` head
//!    accepts it outright (the official's
//!    `acceptsFunctionType(context) && getCallMethodType` arm).
//! 4. The ordinary nullability verdict: a `T?`/`Null` source into a
//!    non-nullable `T` formal is `Left Nullable`'s definite rejection.
//!
//! Oracle provenance: the #2921 census rows —
//! `ArgumentTypeNotAssignableTest/test_functionType#0`,
//! `test_invocation_functionTypes_optional#0`,
//! `test_invocation_functionTypes_optional_method#0`,
//! `BinaryExpressionResolutionTest/test_plus_int_never#0`,
//! `NonErrorResolverTest/test_argumentTypeNotAssignable_classWithCall_
//! Function#0`, `StrongModeLocalInferenceTest/test_constrainedByBounds5#0`,
//! and the `never_null_assignability` pair (`dart analyze`, Dart 3.13).

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

// —— scratch probes: the four faces' minimal reproductions ——

/// B2 probe: `a + b` with `b: Never` — the oracle's bottom acceptance
/// keeps the right operand silent.
#[test]
fn probe_plus_int_never_silent() {
    let report = report("f(int a, Never b) {\n\x20 a + b;\n}\n");
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert!(rows.is_empty(), "Never rhs stays silent: {rows:?}");
}

/// B2 probe: `takesNull(never)` — the `Null` formal accepts the bottom
/// source outright.
#[test]
fn probe_takes_null_never_silent() {
    let report = report(
        "void takesNull(Null n) {}\n\
         Never never = throw '';\n\
         void f() {\n\
         \x20 takesNull(never);\n\
         }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert!(rows.is_empty(), "Never into Null stays silent: {rows:?}");
}

/// A3 probe: `a.n(() => 0)` over `n(void f(int i))` — the closure's
/// `void Function()` against `void Function(int)` reports.
#[test]
fn probe_function_type_arity() {
    let report = report(
        "m() {\n\
         \x20 var a = new A();\n\
         \x20 a.n(() => 0);\n\
         }\n\
         class A {\n\
         \x20 n(void f(int i)) {}\n\
         }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "the arity face reports: {rows:?}");
    let (_, _, message) = &rows[0];
    // The oracle's machine-format row ends at `'. ` — the empty
    // `#additionalInfo` appendix keeps its separating space (live 3.13.7
    // `dart analyze --format=machine`, byte-confirmed by od).
    assert_eq!(
        message,
        "The argument type 'void Function()' can't be assigned to the \
         parameter type 'void Function(int)'. ",
        "the arity message face: {rows:?}"
    );
}

/// A3 probe: `acceptFunOptBool(funBool)` — `void Function(bool)` into
/// `void Function([bool])` reports.
#[test]
fn probe_required_into_optional() {
    let report = report(
        "void acceptFunOptBool(void funNumOptBool([bool b])) {}\n\
         void funBool(bool b) {}\n\
         main() {\n\
         \x20 acceptFunOptBool(funBool);\n\
         }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "the optional face reports: {rows:?}");
}

/// B5 probe: `caller(new CallMeBack())` over `caller(Function callee)`
/// — the `call`-carrying class assigns into `Function`.
#[test]
fn probe_call_class_into_function() {
    let report = report(
        "caller(Function callee) {\n\
         \x20 callee();\n\
         }\n\
         class CallMeBack {\n\
         \x20 call() => 0;\n\
         }\n\
         main() {\n\
         \x20 caller(new CallMeBack());\n\
         }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert!(
        rows.is_empty(),
        "the call-class assigns into Function: {rows:?}"
    );
}

/// A1 probe: `useNonNullable(i)` with `i: int?` — the nullable source
/// into a non-nullable formal reports.
#[test]
fn probe_nullable_into_nonnullable() {
    let report = report(
        "void useNonNullable(int i) {}\n\
         void f(int? i) {\n\
         \x20 useNonNullable(i);\n\
         }\n",
    );
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "the nullable face reports: {rows:?}");
}
