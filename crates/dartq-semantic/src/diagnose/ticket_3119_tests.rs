//! The #3119 pins: `unchecked_use_of_nullable_value`'s statement-form
//! operand faces — the `yield*` operand arm
//! (`uncheckedUseOfNullableValueInYieldEach`) and the declared-type
//! fallback for the operands the flow read table cannot spell.
//!
//! Upstream citations:
//! - the `yield*` arm is `NullableDereferenceVerifier.expression` gated on
//!   `node.star != null` at `.at(expression)` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/yield_statement_resolver.dart:163-168);
//!   `void` is `isPotentiallyNullable` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/error/nullable_dereference_verifier.dart's type
//!   switch — the `VoidType` arm).
//! - the for-in iterable (`uncheckedUseOfNullableValueAsIterator`) and the
//!   spread operand (`uncheckedUseOfNullableValueInSpread`) ask the same
//!   verifier (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/for_resolver.dart:124 and
//!   pkg/analyzer/lib/src/generated/type_analyzer.dart's
//!   `visitSpreadElement` `!node.isNullAware` arm).
//! - a `?.`-merged access short-circuits to `null`: its own static type
//!   re-wraps the member's answer nullable (`a?.foo` over `a: A?`,
//!   `foo: bool` reads `bool?`), so a `!` operand reports the
//!   nullable-condition face `uncheckedUseOfNullableValueAsCondition`
//!   (upstream Dart SDK source, pkg/analyzer/messages.yaml
//!   `uncheckedUseOfNullableValue` shared-name family;
//!   bool_expression_verifier.dart's `checkForNonBoolNegationExpression`
//!   arm).

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

fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// `yield*` on a nullable operand reports
/// `uncheckedUseOfNullableValueInYieldEach` at the operand (upstream Dart
/// SDK source, yield_statement_resolver.dart:163-168).
#[test]
fn yield_each_nullable_operand_reports() {
    let report = report(
        "class Iterable<T> {}\n\
         m() sync* {\n\
         \x20 Iterable<int>? x;\n\
         \x20 yield* x;\n\
         }",
    );
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", codes(&report));
    assert_eq!(
        rows[0].message, "A nullable expression can't be used in a yield-each statement."
    );
}

/// A promoted `yield*` operand (`x` proven non-null in the guard) stays
/// silent — the flow's read row substitutes the promoted type at the
/// teardown join.
#[test]
fn yield_each_promoted_operand_stays_silent() {
    let report = report(
        "class Iterable<T> {}\n\
         m(Iterable<int>? x) sync* {\n\
         \x20 if (x != null) {\n\
         \x20\x20 yield* x;\n\
         \x20 }\n\
         }",
    );
    assert!(
        !codes(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        codes(&report)
    );
}

/// A bare `yield` (no `*`) asks no operand face — a nullable yielded
/// element is `yield_of_invalid_type`'s question, never the verifier's.
#[test]
fn bare_yield_asks_no_operand_face() {
    let report = report(
        "class Iterable<T> {}\n\
         m() sync* {\n\
         \x20 int? x;\n\
         \x20 yield x;\n\
         }",
    );
    assert!(
        !codes(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        codes(&report)
    );
}

/// The pseudo-keyword's identifier reading (`yield` a local variable,
/// `yield * n` a multiplication) is no yield-each — no operand row.
#[test]
fn yield_as_identifier_is_no_yield_each() {
    let report = report(
        "m() {\n\
         \x20 var yield = 2;\n\
         \x20 var z = yield * 3;\n\
         }",
    );
    assert!(
        !codes(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        codes(&report)
    );
}

/// A `void`-typed formal's for-in iterable stays silent on the
/// unchecked-use face — the operand's `ReadType` row drops at the flow
/// staging boundary (`FlowBase::Void` is unrepresentable) and its pass-1
/// row is consumed by teardown, so the declared `void` face is
/// unreachable from this join. The oracle still reports
/// `unchecked_use_of_nullable_value` beside its `use_of_void_result`
/// (forIn_iterable_typeVoid); that face is a residual the assignability
/// pass that owns `use_of_void_result` must stage — outside this
/// teardown-join arm. The pin locks the current under-report (no wrong
/// code emitted), never the oracle's row.
#[test]
fn for_in_void_iterable_under_reports() {
    let report = report(
        "void f(void x, y) {\n\
         \x20 for (y in x) {}\n\
         }",
    );
    assert!(
        !codes(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        codes(&report)
    );
}

/// A `void`-typed formal's spread operand under-reports the same way —
/// the `void` face is unreachable at teardown.
#[test]
fn spread_void_operand_under_reports() {
    let report = report(
        "void f(void x) {\n\
         \x20 var y = [...x];\n\
         }",
    );
    assert!(
        !codes(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        codes(&report)
    );
}

/// A `void`-typed `yield*` operand under-reports the same way.
#[test]
fn yield_each_void_operand_under_reports() {
    let report = report(
        "void f(void x) sync* {\n\
         \x20 yield* x;\n\
         }",
    );
    assert!(
        !codes(&report).contains(&"unchecked_use_of_nullable_value"),
        "{:?}",
        codes(&report)
    );
}

/// `!a?.foo` over `a: A?`, `foo: bool`: the `?.`-merged access's own
/// static type is `bool?`, so the negation operand stages the
/// nullable-condition row (upstream's
/// `uncheckedUseOfNullableValueAsCondition` at the operand extent).
#[test]
fn bang_null_aware_member_reports_condition() {
    let report = report(
        "class A {\n\
         \x20 bool get foo => true;\n\
         }\n\
         void f(A? a) {\n\
         \x20 !a?.foo;\n\
         }",
    );
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", codes(&report));
    assert_eq!(
        rows[0].message, "A nullable expression can't be used as a condition."
    );
}

/// `a?.foo` on a NON-nullable `a` still reads `bool?` — the `?.`
/// short-circuit face wraps the member's answer regardless of the
/// receiver's own nullability (the oracle's `a?.foo` over `A` reports the
/// nullable-condition row beside its `invalid_null_aware_operator`
/// warning).
#[test]
fn bang_null_aware_on_non_nullable_still_nullable() {
    let report = report(
        "class A {\n\
         \x20 bool get foo => true;\n\
         }\n\
         void f(A a) {\n\
         \x20 !a?.foo;\n\
         }",
    );
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", codes(&report));
    assert_eq!(
        rows[0].message, "A nullable expression can't be used as a condition."
    );
}
