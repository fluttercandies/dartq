//! #3210 (F1) — the aux-file `[]` receiver face in the materialized co19
//! posture, pinned.
//!
//! `expect_common.dart` is a `part of 'expect.dart'` unit whose
//! `Expect.deepEquals` declares local functions over untyped parameters;
//! the `[]` reads inside them (`plan2check(expected[key], actual[key])` at
//! aux lines 316/320) must resolve — the `is`-narrowed parameter through
//! the promoted `Map`/`List` interface, the unnarrowed sibling through
//! dynamic dispatch — and never cascade `undefined_operator` on an empty
//! receiver display. That cascade (`The operator '[]' isn't defined for
//! the type ''.` × 2 rows in every co19 case pulling the aux file) was the
//! wave-B F1 board face; its production site (the empty-named carrier the
//! is-narrow staging used to mint) was removed with #3058 —
//! `staged_promotion_typed` (types.rs) interns the flow name's real
//! lexeme, and the empty-named face cannot arise (`name_lexeme`'s ids are
//! dense; synthetic tokens read as their `#<handle>` placeholder). These
//! pins lock the family silent in the exact materialized posture — a
//! part-of unit diagnosed inside the `expect` library over an indexed
//! core — plus the negative faces: a genuinely absent spelling still
//! reports, through promotion and without it.

use super::*;
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use crate::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder,
    Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The names-only `dart:core` stub: the special spellings the engine
/// decides by name (`Never` is the bottom type, `Object` the root). The
/// member-bearing core classes ride the root unit below — the production
/// SDK projection indexes `dart:core` from source, so `Map`/`List`/`int`
/// are indexed containers there (engine `sdk_units.rs`: "real members and
/// signatures, real supertype edges, no hand-kept face table"), and the
/// stub mirrors that grade over the members the fixture spells.
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// The co19 `Utils/expect.dart` root unit: the library directive, the
/// part directive, and the indexed core stubs carrying every member the
/// `deepEquals` family spells (`Map`'s `[]`/`[]=`/`remove`/`keys`,
/// `List`'s `[]`/`length`, `Iterable`'s `isEmpty`/`first`, `int`'s
/// arithmetic and comparison face).
fn expect_root() -> String {
    "library expect;\n\
     part 'expect_common.dart';\n\
     class num {}\n\
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; bool operator <(int o) => true; bool operator >(int o) => true; }\n\
     class bool {}\n\
     class String {}\n\
     class Iterable<E> { bool get isEmpty => true; E get first => throw 0; }\n\
     class List<E> extends Iterable<E> { E operator [](int i) => throw 0; void operator []=(int i, E v) {} int get length => 0; }\n\
     class Map<K, V> { V? operator [](K k) => null; void operator []=(K k, V v) {} Iterable<K> get keys => throw 0; V? remove(K k) => null; }\n"
        .to_string()
}

/// Diagnoses `part_source` (a `part of` unit) inside the library whose
/// root `root_source` declares `library expect; part 'expect_common.dart';`
/// — the materialized co19 aux-file posture: the `Expect` class lives in
/// the PART unit, the root owns the library's imports and the indexed
/// core stubs, and both units' clause trees feed the resolved-type
/// snapshot exactly the way the engine mounts it.
fn report_part(root_source: &str, part_source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let root_file = files.intern(Path::new("expect.dart"));
    let part_file = files.intern(Path::new("expect_common.dart"));
    let scan_cfg = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let root_scanned = dartq_scanner::scan(root_source, scan_cfg, root_file);
    assert!(
        root_scanned.diagnostics.is_empty(),
        "scanner diagnostics: {root_source:?}"
    );
    let part_scanned = dartq_scanner::scan(part_source, scan_cfg, part_file);
    assert!(
        part_scanned.diagnostics.is_empty(),
        "scanner diagnostics: {part_source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let root_elements = collect(root_source, &root_scanned.tokens, features, &strings);
    let part_elements = collect(part_source, &part_scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:expect/expect.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(root_file, root_elements.clone());
    index.insert(part_file, part_elements.clone());
    builder.unit(lib, root_file);
    builder.unit(lib, part_file);
    let mut clauses = ClauseTrees::empty();
    for (unit_file, unit_source, unit_scanned, unit_elements) in [
        (root_file, root_source, &root_scanned, &root_elements),
        (part_file, part_source, &part_scanned, &part_elements),
    ] {
        for (name, decl) in unit_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            if matches!(
                decl.kind,
                MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
            ) {
                builder.declare_class(lib, name, unit_file, ClassModifiers::default());
            } else {
                builder.declare(lib, name, unit_file);
            }
        }
        crate::capture_clause_trees(
            unit_file,
            unit_source,
            &unit_scanned.tokens,
            features,
            &strings,
            &mut clauses,
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types =
        LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, lib, &clauses);
    let ops = ResolvedTypeOps::new(&types, &strings, part_file);
    let fold = fold_unit_with_ops(
        part_source,
        &part_scanned.tokens,
        features,
        &strings,
        part_file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// `code @offset: message` per row.
fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// The `undefined_operator` messages in source order.
fn operator_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `deepEquals` family verbatim (the materialized aux file's shape):
/// static method, untyped parameters, local functions shadowing them,
/// `is`-narrowed if/else-if chains, index reads on both the narrowed and
/// the unnarrowed parameter feeding a call — the exact lines 316/320
/// carriers.
const DEEP_EQUALS: &str = "class Expect {\n\
     \x20 static void equalsOrNaN(expected, actual, [String reason = '']) {}\n\
     \x20 static void isNull(actual, [String reason = '']) {}\n\
     \x20 static void fail(String message) {}\n\
     \x20 static void deepEquals(expected, actual, [String reason = '']) {\n\
     \x20   Map planned = Map();\n\
     \x20   Map processed = Map();\n\
     \n\
     \x20   void plan2check(expected, actual) {\n\
     \x20     if (expected == null) {\n\
     \x20       Expect.isNull(actual);\n\
     \x20     }\n\
     \n\
     \x20     if ((expected is Map) || (expected is List)) {\n\
     \x20       var savedActual = planned[expected];\n\
     \x20       if (savedActual != null) {\n\
     \x20         Expect.equalsOrNaN(savedActual, actual);\n\
     \x20       } else if ((savedActual = processed[expected]) != null) {\n\
     \x20         Expect.equalsOrNaN(savedActual, actual);\n\
     \x20       } else {\n\
     \x20         Expect.equalsOrNaN(expected.length, actual.length, 'lengths');\n\
     \x20         planned[expected] = actual;\n\
     \x20       }\n\
     \x20     } else {\n\
     \x20       Expect.equalsOrNaN(expected, actual, reason);\n\
     \x20     }\n\
     \x20   }\n\
     \n\
     \x20   void runPlanned(expected, actual) {\n\
     \x20     if (expected is Map) {\n\
     \x20       for (var key in expected.keys) {\n\
     \x20         plan2check(expected[key], actual[key]);\n\
     \x20       }\n\
     \x20     } else if (expected is List) {\n\
     \x20       for (int i = 0; i != expected.length; i++) {\n\
     \x20         plan2check(expected[i], actual[i]);\n\
     \x20       }\n\
     \x20     } else {\n\
     \x20       Expect.fail(\"only Lists and Maps expected in the plan\");\n\
     \x20     }\n\
     \x20     planned.remove(expected);\n\
     \x20     processed[expected] = actual;\n\
     \x20   }\n\
     \n\
     \x20   try {\n\
     \x20     plan2check(expected, actual);\n\
     \x20     for (;;) {\n\
     \x20       Iterable keys = planned.keys;\n\
     \x20       if (keys.isEmpty) {\n\
     \x20         return;\n\
     \x20       }\n\
     \x20       var key = keys.first;\n\
     \x20       runPlanned(key, planned[key]);\n\
     \x20     }\n\
     \x20   } catch (error) {\n\
     \x20     rethrow;\n\
     \x20   }\n\
     \x20 }\n\
     }\n";

/// The verbatim `deepEquals` shape as a part unit of the `expect`
/// library (the materialized co19 aux-file posture): every `[]`/`[]=`
/// read and write resolves — the F1 lines 316/320 stay silent, promoted
/// face and dynamic sibling alike.
#[test]
fn deep_equals_part_unit_stays_silent() {
    let part = format!("part of 'expect.dart';\n{DEEP_EQUALS}");
    let report = report_part(&expect_root(), &part);
    assert!(report.diagnostics.is_empty(), "rows: {:#?}", rows(&report));
}

/// The distilled 316/320 face alone: the if/else-if chain over an
/// untyped parameter of a local function in a static method, index reads
/// on both parameters feeding the call.
#[test]
fn run_planned_index_reads_stay_silent() {
    let part = "part of 'expect.dart';\n\
         class Expect {\n\
         \x20 static void deepEquals(expected, actual) {\n\
         \x20   void plan2check(a, b) {}\n\
         \x20   void runPlanned(expected, actual) {\n\
         \x20     if (expected is Map) {\n\
         \x20       for (var key in expected.keys) {\n\
         \x20         plan2check(expected[key], actual[key]);\n\
         \x20       }\n\
         \x20     } else if (expected is List) {\n\
         \x20       for (int i = 0; i != expected.length; i++) {\n\
         \x20         plan2check(expected[i], actual[i]);\n\
         \x20       }\n\
         \x20     }\n\
         \x20   }\n\
         \x20   runPlanned(expected, actual);\n\
         \x20 }\n\
         }\n";
    let report = report_part(&expect_root(), part);
    assert!(report.diagnostics.is_empty(), "rows: {:#?}", rows(&report));
}

/// The unnarrowed half on its own: an untyped local-function parameter's
/// `[]` read dispatches dynamically and stays silent.
#[test]
fn untyped_local_fn_param_index_silent() {
    let part = "part of 'expect.dart';\n\
         class Expect {\n\
         \x20 static void deepEquals(actual) {\n\
         \x20   void runPlanned(expected, actual) {\n\
         \x20     actual[0];\n\
         \x20   }\n\
         \x20   runPlanned(null, actual);\n\
         \x20 }\n\
         }\n";
    let report = report_part(&expect_root(), part);
    assert!(report.diagnostics.is_empty(), "rows: {:#?}", rows(&report));
}

/// The narrowed half on its own: `is Map` re-types the read so `[]`
/// resolves on the promoted interface, `keys` included.
#[test]
fn is_map_promoted_index_silent() {
    let part = "part of 'expect.dart';\n\
         void f(expected) {\n\
         \x20 if (expected is Map) {\n\
         \x20   for (var key in expected.keys) {\n\
         \x20     expected[key];\n\
         \x20   }\n\
         \x20 }\n\
         }\n";
    let report = report_part(&expect_root(), part);
    assert!(report.diagnostics.is_empty(), "rows: {:#?}", rows(&report));
}

/// The `is List` twin with the counted loop.
#[test]
fn is_list_promoted_for_index_silent() {
    let part = "part of 'expect.dart';\n\
         void f(expected) {\n\
         \x20 if (expected is List) {\n\
         \x20   for (int i = 0; i != expected.length; i++) {\n\
         \x20     expected[i];\n\
         \x20   }\n\
         \x20 }\n\
         }\n";
    let report = report_part(&expect_root(), part);
    assert!(report.diagnostics.is_empty(), "rows: {:#?}", rows(&report));
}

/// The invalid-carrier face (the closest constructible proxy for the
/// empty-named receiver): a receiver whose declared type resolves nowhere
/// answers the `[]` ask silently — only the type-position miss reports,
/// the index never cascades `undefined_operator`.
#[test]
fn undeclared_receiver_index_never_cascades() {
    let part = "part of 'expect.dart';\nvoid f(Gone g) { g[0]; }\n";
    let report = report_part(&expect_root(), part);
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>(),
        ["undefined_class"],
        "rows: {:#?}",
        rows(&report)
    );
}

/// Negative (oracle-matching): `[]` on `int` still reports naming `int` —
/// a genuine miss through an indexed container face, empty display never.
#[test]
fn int_index_still_reports_named_type() {
    let report = report_part(
        &expect_root(),
        "part of 'expect.dart';\nvoid f(int x) { x[0]; }\n",
    );
    assert_eq!(
        operator_messages(&report),
        vec!["The operator '[]' isn't defined for the type 'int'.".to_string()]
    );
}

/// Negative (oracle-matching): the promoted face's own miss still
/// reports — `d is int` then `d[0]` names `int`, never `''` and never a
/// dynamic silence.
#[test]
fn is_int_promoted_index_still_reports_named_type() {
    let report = report_part(
        &expect_root(),
        "part of 'expect.dart';\nvoid f(dynamic d) {\n  if (d is int) {\n    d[0];\n  }\n}\n",
    );
    assert_eq!(
        operator_messages(&report),
        vec!["The operator '[]' isn't defined for the type 'int'.".to_string()]
    );
}
