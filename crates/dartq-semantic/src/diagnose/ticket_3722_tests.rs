//! #3722 pins — the InvalidType-scrutinee gate's decidable arm reads the
//! ORACLE's `InvalidType` production, not `resolve_container`'s stricter
//! "indexed container face" answer.
//!
//! Upstream latches `patternConverter.hasInvalidType` on
//! `scrutineeType is InvalidType` (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/constant/constant_verifier.dart:1042`) and
//! `InvalidType` is the resolver's answer when the type name's scope
//! lookup misses, hits a `MultiplyDefinedElement`, or lands a non-type
//! element (named_type_resolver.dart:201-202, :525-526). A name that
//! resolves to a real declaration — including one whose unit was never
//! analyzed, the test rigs' names-only `dart:core` stub — yields a real
//! `InterfaceType`, and the exhaustiveness pass runs on it.
//!
//! The gate's first cut asked `resolve_container`, which additionally
//! requires an [`ElementIndex`] face: a `Found(Decl(owner))` whose owner
//! never entered the index answered `None`, so the names-only `Object`
//! stub read as `InvalidType` and its switch's rows dropped. The decidable
//! arm now treats an unindexed owner as resolved and drops only the shapes
//! upstream's `InvalidType` actually covers (a namespace miss, an
//! ambiguous name, a prefix, or an indexed-but-non-type declaration).

use super::test_rig::{code_names, report_ops};
use crate::diagnose::FileReport;
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The exhaustiveness family the gate owns (the three codes whose oracle
/// producer sits behind `hasInvalidType`).
fn family_codes(report: &FileReport) -> Vec<&'static str> {
    const FAMILY: [&str; 3] = [
        "non_exhaustive_switch_statement",
        "non_exhaustive_switch_expression",
        "unreachable_switch_case",
    ];
    report
        .diagnostics
        .iter()
        .filter(|d| FAMILY.contains(&d.code.name))
        .map(|d| d.code.name)
        .collect()
}

/// The #3525 repro: an `Object?` scrutinee whose head resolves to the
/// names-only `dart:core` stub — a real declaration upstream, so the
/// exhaustiveness verdict stands and the second `Null _` arm reports
/// `unreachable_switch_case`. The pre-fix gate read the unindexed `Object`
/// stub as `InvalidType` and dropped the row.
#[test]
fn object_stub_scrutinee_duplicate_null_case_reports_unreachable() {
    let report = report_ops(
        "void f(Object? o) { switch (o) { case Null _: break; case Null _: break; } }",
    );
    assert_eq!(
        family_codes(&report),
        ["unreachable_switch_case"],
        "{:?}",
        family_codes(&report)
    );
    // The oracle anchors the row at the duplicate's `case` keyword.
    let analyzed = format!(
        "{}void f(Object? o) {{ switch (o) {{ case Null _: break; case Null _: break; }} }}",
        super::test_rig::CORE_PRELUDE,
    );
    let second_case = analyzed.rfind("case").unwrap() as u32;
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "unreachable_switch_case")
        .expect("the unreachable row");
    assert_eq!(row.range.offset, second_case, "{row:?}");
    assert_eq!(row.range.length, 4, "{row:?}");
}

/// A genuinely exhaustive `Object?` switch stays silent: `Null _` covers
/// `Null` and `Object _` covers the non-null remainder — the gate keeps
/// the verdict AND the verdict was exhaustive.
#[test]
fn object_stub_scrutinee_exhaustive_stays_silent() {
    let report = report_ops(
        "void f(Object? o) { switch (o) { case Null _: break; case Object _: break; } }",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The expression face keeps its reachability row the same way: the
/// second `Null _` arm of a `=>` switch over the `Object?` stub head is
/// `unreachable_switch_case` (probed `dart analyze` — oracle reports it
/// alongside the `Object()` covering gap and the never-matches/dead-code
/// rows; the covering row itself is an engine-side under-report the gate
/// never produced, outside this ticket).
#[test]
fn object_stub_scrutinee_expression_duplicate_reports_unreachable() {
    let report = report_ops(
        "int e(Object? o) => switch (o) { Null _ => 1, Null _ => 2 };",
    );
    assert_eq!(
        family_codes(&report),
        ["unreachable_switch_case"],
        "{:?}",
        family_codes(&report)
    );
}

/// The preserved-suppression control (#2336's own face): a scrutinee head
/// that resolves NOWHERE in the diagnosing scope is still the oracle's
/// `InvalidType` — the namespace lookup misses outright, so the gate drops
/// the whole family. The dependency is declared and indexed (the corpus'
/// unique-declaration fallback posture) but never imported.
#[test]
fn unimported_scrutinee_still_suppresses() {
    let report = switch_report(
        "import 'missing.dart'; int f(K k) { switch (k) { case K.a: return 1; case K.b: return 2; } }",
        "enum K { a, b }",
        false,
    );
    assert!(
        family_codes(&report).is_empty(),
        "{:?}",
        family_codes(&report)
    );
}

/// The same unimported shape with the import intact keeps the ordinary
/// verdict (the unindexed-owner arm never fires — `K` resolves to a real
/// indexed container), proving the narrowed predicate did not leak rows
/// for resolved heads either: the duplicate `K.a` arm reports
/// `unreachable_switch_case`.
#[test]
fn imported_scrutinee_keeps_its_verdict() {
    let report = switch_report(
        "import 'kit.dart'; int f(K k) { switch (k) { case K.a: return 1; case K.a: return 1; } }",
        "enum K { a, b }",
        true,
    );
    // The duplicate `K.a` makes `K.b` uncovered (the covering row) AND the
    // second arm unreachable — both rows the oracle emits.
    assert_eq!(
        family_codes(&report),
        ["non_exhaustive_switch_statement", "unreachable_switch_case"],
        "{:?}",
        family_codes(&report)
    );
}

/// The two-library rig (the #2336 pins' own): `dependency` is analyzed and
/// declared into the graph, but the main unit's import of it rides a
/// broken `missing.dart` path unless `imported` — the namespace face
/// cannot resolve `K` while the fallback can.
fn switch_report(main_source: &str, dependency: &str, imported: bool) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        main_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {main_source:?}"
    );
    let scanned_dep = dartq_scanner::scan(
        dependency,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dep_file,
    );
    assert!(
        scanned_dep.diagnostics.is_empty(),
        "scanner diagnostics: {dependency:?}"
    );
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let dep_elements = collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern("package:t/kit.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in dep_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(dep_lib, name, dep_file);
    }
    if imported {
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: None,
                deferred: false,
                non_library: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    crate::diagnose::diagnose_with(
        main_source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        crate::diagnose::StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// The #2340 dead-case synthesizer shares the invalid-scrutinee posture:
/// a resolvable-type clause (`int i`) inside an `Object`-stub-head switch
/// is the absorber shape — oracle emits nothing dead here, so the whole
/// report keeps to the pattern verdicts alone (no synthesized
/// `dead_code`).
#[test]
fn object_stub_scrutinee_resolvable_clause_synthesizes_no_dead_rows() {
    let report = report_ops(
        "void k(Object? o) { switch (o) { case int i: break; case Null _: break; } }",
    );
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert!(dead.is_empty(), "{dead:?}");
}

/// A non-`Object` resolved head keeps the gate's pre-existing behavior
/// byte-identical: the enum's own unit declares it, the unindexed-owner
/// arm never fires, and the missing constant reports
/// `non_exhaustive_switch_statement` at `switch`.
#[test]
fn non_object_named_scrutinee_verdict_still_decides() {
    // A declared, resolved enum head keeps the gate's pre-existing
    // behavior exactly: the missing constant reports at `switch`.
    let report = report_ops(
        "enum K { a, b } int f(K k) { switch (k) { case K.a: return 1; } }",
    );
    assert_eq!(
        family_codes(&report),
        ["non_exhaustive_switch_statement"],
        "{:?}",
        code_names(&report)
    );
}
