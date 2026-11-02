//! The #2843 R4 pins: the three unchecked-use faces the pattern/ctor wave
//! added — the relational-pattern gate (the matched value's nullable read
//! at the operator token), the object-pattern nullable-matched head (the
//! field reads at the pattern's type name), and the extension-type
//! primary-constructor `this{}` block's `Object?` parameter default.
//!
//! Oracle rows: dart 3.13.7 (the pinned conformance oracle) over the
//! census cases (`UncheckedUseOfNullableValueTest`
//! test_uncheckedOperatorInvocation_relationalPattern{,_hasExtension},
//! co19 `type_inference_A21_t05`'s twelve operator rows,
//! `ObjectPatternResolutionTest` test_typeAlias_nullable, co19
//! `nullable_object_pattern_error_test`, co19 Primary-constructors
//! `static_processing_A12_t07`'s four this-block rows). Official sources:
//! resolver.dart `resolveRelationalPatternOperator` (`propertyErrorEntity:
//! node.operator`), type_analyzer.dart `analyzeObjectPattern`
//! (`receiverType: requiredType`, `propertyErrorEntity: objectPattern.type`),
//! type_property_resolver.dart's nullable arm (Object lookup, extension
//! lookup, `_reportedGetterError` suppression), and the representation
//! formal's `Object?` default (the implicit-`final` declaring formal).

use super::*;
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The member-bearing core classes a fixture may name (the shared rig's
/// prelude grade): declared ahead of the fixture so every core spelling
/// resolves.
const CORE_PRELUDE: &str = "class num {} \
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; bool operator >(int o) => true; bool operator <(int o) => true; } \
     class double extends num {} \
     class String { String operator +(String other) => this; int get length => 0; } class bool {} class Iterable<E> {} \
     class List<E> extends Iterable<E> { void add(E e) {} E operator [](int i) => throw 0; int get length => 0; } \
     class Map<K, V> { V? operator [](K k) => null; Iterable<K> get keys => throw 0; } \
     class Set<E> extends Iterable<E> {} class Exception {} \
     abstract class Enum { int get index => 0; }\n";

/// The names-only `dart:core` stub: the special spellings the engine
/// decides by name (`Never` is the bottom type, `Object` the root).
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// Diagnoses `source` (the sole analyzed file of `package:t`, the
/// names-only `dart:core` stub imported) with the resolved-type snapshot
/// mounted. `primary_constructors` layers the experiment opt-in the
/// `this{}`-block fixtures need.
fn unit_report(source: &str, primary_constructors: bool) -> FileReport {
    let full = format!("{CORE_PRELUDE}{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let set = if primary_constructors {
        set.with_experiment(Feature::PrimaryConstructors)
    } else {
        set
    };
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        &full,
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
    let elements = collect(&full, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        dartq_resolution::ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: dartq_resolution::Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        if matches!(
            decl.kind,
            MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
        ) {
            builder.declare_class(lib, name, file, dartq_resolution::ClassModifiers::default());
        } else {
            builder.declare(lib, name, file);
        }
    }
    let mut clauses = crate::resolved_ops::ClauseTrees::empty();
    crate::capture_clause_trees(
        file,
        &full,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = crate::resolved_ops::LibraryTypes::build_with_clauses(
        &graph,
        &namespaces,
        &index,
        &strings,
        lib,
        &clauses,
    );
    let ops = crate::resolved_ops::ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
        &full,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &dartq_resolution::LibraryClassModifiers::default(),
        set.is_enabled(Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// The `unchecked_use_of_nullable_value` rows as `offset:len message`.
fn unchecked(report: &FileReport) -> Vec<(usize, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| {
            (
                d.range.offset as usize,
                d.range.length,
                d.message.to_string(),
            )
        })
        .collect()
}

/// Exactly one unchecked row at `offset:len` carrying `message`.
#[track_caller]
fn assert_one_unchecked(report: &FileReport, offset: usize, len: u32, message: &str) {
    let rows = unchecked(report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, offset, "offset");
    assert_eq!(rows[0].1, len, "length");
    assert_eq!(rows[0].2, message);
}

// —— the relational-pattern gate —————————————————————————————————————————

/// A relational pattern over a nullable matched value reports at the
/// operator token, its length the lexeme's width (the oracle's
/// test_uncheckedOperatorInvocation_relationalPattern: `if (x case > 0)`
/// anchors at `>`).
#[test]
fn relational_over_nullable_scrutinee_reports_at_operator() {
    let src = "void f(int? x) {\n  switch (x) {\n    case > 0:\n      break;\n  }\n}\n";
    let report = unit_report(src, false);
    assert_one_unchecked(
        &report,
        at(src, "> 0"),
        1,
        "The operator '>' can't be unconditionally invoked because the receiver can be 'null'.",
    );
}

/// Every comparison operator reports; a two-character lexeme takes its own
/// width (the co19 type_inference_A21_t05 shape over an `A?` alias).
#[test]
fn relational_operator_family_reports_with_lexeme_widths() {
    let src = "typedef A = int;\n\
               String test1(A? a) {\n  switch (a) {\n    case < 0:\n      return '<';\n\
               \x20   case <= 0:\n      return '<=';\n    case > 0:\n      return '>';\
               \n    default:\n      return 'default';\n  }\n}\n";
    let report = unit_report(src, false);
    let rows = unchecked(&report);
    assert_eq!(rows.len(), 3, "{rows:?}");
    let lt = at(src, "< 0");
    let le = at(src, "<= 0");
    let gt = at(src, "> 0");
    assert_eq!(rows[0], (lt, 1, rows[0].2.clone()));
    assert_eq!(rows[1], (le, 2, rows[1].2.clone()));
    assert_eq!(rows[2], (gt, 1, rows[2].2.clone()));
    for row in &rows {
        assert!(
            row.2.starts_with("The operator '"),
            "operator face: {}",
            row.2
        );
    }
}

/// An if-case scrutinee gates the same way (the matched type is the
/// if-case's own context).
#[test]
fn if_case_relational_reports_at_operator() {
    let src = "void f(int? x) {\n  if (x case > 0) {}\n}\n";
    let report = unit_report(src, false);
    assert_eq!(unchecked(&report).len(), 1);
    assert_eq!(unchecked(&report)[0].0, at(src, "> 0"));
}

/// The or/and operands keep the scrutinee as their matched type — each
/// operand reports at its own operator.
#[test]
fn logical_or_operands_keep_scrutinee_matched_type() {
    let src = "void f(int? x) {\n  if (x case > 0 || < 5) {}\n}\n";
    let report = unit_report(src, false);
    let rows = unchecked(&report);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, at(src, "> 0"));
    assert_eq!(rows[1].0, at(src, "< 5"));
}

/// `==`/`!=` never report: both spell `operator ==`, which every type
/// resolves on `Object` (the official methodName rewrite).
#[test]
fn equality_operators_stay_silent() {
    let src = "void f(int? x) {\n  switch (x) {\n    case == 0:\n      break;\n\
               \x20   case != 0:\n      break;\n  }\n}\n";
    let report = unit_report(src, false);
    assert!(unchecked(&report).is_empty(), "{:?}", unchecked(&report));
}

/// An applicable extension operator resolves the read silent (the oracle's
/// test_uncheckedOperatorInvocation_relationalPattern_hasExtension: the
/// `operator >` on `int?` face wins the nullable bucket).
#[test]
fn extension_operator_on_nullable_face_resolves() {
    let src = "void f(int? x) {\n  if (x case > 0) {}\n}\n\
               extension E on int? {\n  bool operator >(int other) => true;\n}\n";
    let report = unit_report(src, false);
    assert!(unchecked(&report).is_empty(), "{:?}", unchecked(&report));
}

/// A non-nullable scrutinee stays silent; a pattern nested inside a record
/// field reads a property column the name level never descends into and
/// stays silent with it.
#[test]
fn non_nullable_and_nested_shapes_stay_silent() {
    let bare = "void f(int x) {\n  if (x case > 0) {}\n}\n";
    let report = unit_report(bare, false);
    assert!(unchecked(&report).is_empty());
    let nested = "void f(int? x) {\n  if (x case (> 0,)) {}\n}\n";
    let report = unit_report(nested, false);
    assert!(unchecked(&report).is_empty());
}

// —— the object-pattern nullable-matched head —————————————————————————————

/// A typedef-aliased nullable head reads each field over a
/// potentially-nullable receiver and reports at the pattern's type name
/// (the oracle's test_typeAlias_nullable: `case A(foo: 0)` anchors at `A`).
/// The found-nothing row never pairs with `undefined_getter`.
#[test]
fn nullable_typedef_head_reports_at_type_name() {
    let src = "typedef A = int?;\n\nvoid f(x) {\n  switch (x) {\n    case A(foo: 0):\n      break;\n  }\n}\n";
    let report = unit_report(src, false);
    assert_one_unchecked(
        &report,
        at(src, "A(foo"),
        1,
        "The property 'foo' can't be unconditionally accessed because the receiver can be 'null'.",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "undefined_getter"),
        "no undefined_getter beside the nullable row"
    );
}

/// A type parameter through its nullable bound reads the same face (the
/// co19 nullable_object_pattern_error_test's `T extends int?` shape).
#[test]
fn nullable_bound_type_parameter_head_reports() {
    let src =
        "void g<T extends int?>(x) {\n  switch (x) {\n    case T(foo: 0):\n      break;\n  }\n}\n";
    let report = unit_report(src, false);
    assert_one_unchecked(
        &report,
        at(src, "T(foo"),
        1,
        "The property 'foo' can't be unconditionally accessed because the receiver can be 'null'.",
    );
}

/// The nullable arm's silences: no fields reads nothing, `Object` members
/// resolve, `==` resolves.
#[test]
fn object_nullable_silences() {
    let src = "typedef A = int?;\n\nvoid f(x) {\n  switch (x) {\n    case A():\n      break;\n\
               \x20   case A(:var runtimeType):\n      break;\n    case A(hashCode: 1):\n      break;\n  }\n}\n";
    let report = unit_report(src, false);
    assert!(unchecked(&report).is_empty(), "{:?}", unchecked(&report));
}

/// An applicable extension getter on the nullable face resolves the field
/// read silent.
#[test]
fn extension_getter_on_nullable_face_resolves() {
    let src = "typedef A = int?;\n\nvoid f(x) {\n  switch (x) {\n    case A(foo: 0):\n      break;\n  }\n}\n\
               extension E on int? {\n  int get foo => 0;\n}\n";
    let report = unit_report(src, false);
    assert!(unchecked(&report).is_empty(), "{:?}", unchecked(&report));
}

/// A non-nullable head keeps the existing faces: the member walk owns the
/// miss (`undefined_getter` at the field name), no unchecked row.
#[test]
fn non_nullable_head_keeps_undefined_getter_face() {
    let src = "class C {}\n\nvoid f(x) {\n  switch (x) {\n    case C(missing: 0):\n      break;\n  }\n}\n";
    let report = unit_report(src, false);
    assert!(unchecked(&report).is_empty(), "{:?}", unchecked(&report));
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_getter")
            .count(),
        1
    );
}

// —— the `this{}` block's `Object?` parameter default —————————————————————

/// An extension type's untyped representation formal reads `Object?` in the
/// this-block body, so a member read over it reports the shared unchecked
/// code at the member name (the co19 static_processing_A12_t07 four-row
/// shape: `x`, `[x]`, `{x}`, `{required x}`).
#[test]
fn this_block_untyped_rep_formal_reads_object_question() {
    let src = "extension type ET1(x) {\n  this {\n    x.testDynamic;\n  }\n}\n\
               extension type ET2([x]) {\n  this {\n    x.testDynamic;\n  }\n}\n\
               extension type ET3({x}) {\n  this {\n    x.testDynamic;\n  }\n}\n\
               extension type ET4({required x}) {\n  this {\n    x.testDynamic;\n  }\n}\n";
    let report = unit_report(src, true);
    let rows = unchecked(&report);
    assert_eq!(rows.len(), 4, "{rows:?}");
    // Each ET body owns one `testDynamic` read; the rows anchor at their
    // own occurrence, in source order.
    let mut expected: Vec<usize> = Vec::new();
    let mut base = CORE_PRELUDE.len();
    while let Some(pos) = src[base - CORE_PRELUDE.len()..].find("testDynamic") {
        expected.push(base + pos);
        base += pos + "testDynamic".len();
    }
    let mut got: Vec<usize> = rows.iter().map(|row| row.0).collect();
    got.sort_unstable();
    assert_eq!(got, expected, "anchors at each member name");
    for row in &rows {
        assert_eq!(row.1, 11, "testDynamic's own width");
        assert_eq!(
            row.2,
            "The property 'testDynamic' can't be unconditionally accessed because the receiver can be 'null'."
        );
    }
}

/// The representation getter carries the same `Object?` face outside the
/// this-block: an instance read through it reports at the next member (the
/// extension_type_error_test `et3.i.arglebargle` shape).
#[test]
fn representation_getter_face_rides_object_question() {
    let src = "extension type ET3(i);\n\nvoid main() {\n  var et3 = ET3(1);\n\
               \x20 if (1 > 2) et3.i.arglebargle;\n}\n";
    let report = unit_report(src, true);
    assert_one_unchecked(
        &report,
        at(src, "arglebargle"),
        11,
        "The property 'arglebargle' can't be unconditionally accessed because the receiver can be 'null'.",
    );
}

/// A written annotation keeps its own face (`int? i` reads nullable); a
/// declared-default representation formal keeps the `dynamic` posture (the
/// spec's declared-default branch — no unchecked row fires over it).
#[test]
fn written_and_default_rep_forms_keep_their_faces() {
    let typed = "extension type ET(int? i) {\n  this {\n    i.foo;\n  }\n}\n";
    let report = unit_report(typed, true);
    assert_eq!(unchecked(&report).len(), 1);
    assert_eq!(unchecked(&report)[0].0, at(typed, "foo"));
    let defaulted = "extension type ET([i = null]);\n\nvoid f(ET et) {\n  et.i.foo;\n}\n";
    let report = unit_report(defaulted, true);
    assert!(unchecked(&report).is_empty(), "{:?}", unchecked(&report));
}
