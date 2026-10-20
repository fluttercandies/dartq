//! The #2336 differential pins: the InvalidType-scrutinee gate over the
//! switch-exhaustiveness rows — upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/constant/constant_verifier.dart:1035/:1095`
//! (`patternConverter.hasInvalidType` gates `computeExhaustiveness`, the
//! per-case reachability walk, and the covering report behind one `if`).
//! A scrutinee whose static type never resolved in the diagnosing scope is
//! that `InvalidType`: the oracle answers silently, while the engine's
//! world (whose unique-declaration fallback resolves sealed-walk names the
//! unit's broken imports never brought in) computed a verdict and emitted.
//! Each fixture shape was probed against oracle `dart analyze` (fvm
//! default) on an isolated broken-import tree before pinning.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The two-library rig with a deliberately broken import: `dependency` is
/// analyzed and declared into the graph (the world's unique-declaration
/// fallback can see it — the corpus posture), but the main unit's
/// `import 'missing.dart';` carries no graph edge unless `imported`, so
/// the namespace face cannot resolve the dependency's names. `main_source`
/// must reference the dependency through that broken directive.
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
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    diagnose_with(
        main_source,
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
    )
}

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

/// The analyzer corpus' `bundle_reader._formalParameterKind` shape: the
/// enum is declared in the graph but the unit's import of it is broken, so
/// the scrutinee's type never resolved in scope. The oracle's InvalidType
/// gate keeps the fully-covered switch silent; the pre-gate engine resolved
/// the enum through the world's fallback while the case constants stayed
/// unresolved, and false-positived the covering row.
#[test]
fn unimported_enum_scrutinee_suppresses_the_covering_row() {
    let report = switch_report(
        "import 'missing.dart'; int f(K k) { switch (k) { case K.a: return 1; case K.b: return 2; } }",
        "enum K { a, b }",
        false,
    );
    assert!(family_codes(&report).is_empty(), "{:?}", family_codes(&report));
}

/// The same shape with the import intact stays the ordinary verdict: a
/// missing constant reports at the keyword (the gate only answers the
/// unresolvable-scrutinee posture, never the covering math).
#[test]
fn imported_enum_missing_constant_still_reports() {
    let report = switch_report(
        "import 'kit.dart'; int f(K k) { switch (k) { case K.a: return 1; } }",
        "enum K { a, b }",
        true,
    );
    assert_eq!(
        family_codes(&report),
        ["non_exhaustive_switch_statement"],
        "{:?}",
        family_codes(&report)
    );
}

/// Full coverage over a resolved enum stays silent (the gate never touches
/// a resolvable scrutinee).
#[test]
fn imported_enum_full_coverage_stays_silent() {
    let report = switch_report(
        "import 'kit.dart'; int f(K k) { switch (k) { case K.a: return 1; case K.b: return 2; } }",
        "enum K { a, b }",
        true,
    );
    assert!(family_codes(&report).is_empty(), "{:?}", family_codes(&report));
}

/// The analyzer corpus' nullability face (the `var origin? => …, null => …`
/// pairs over a member of an unimported type): the broken-import scrutinee
/// reads as the InvalidType shape, so the oracle's gate suppresses the
/// per-case reachability row too. The pre-gate engine latched the
/// as-written named type, judged it non-nullable, and false-positived the
/// first arm's `unreachable_switch_case`.
#[test]
fn unimported_nullable_scrutinee_suppresses_the_unreachable_row() {
    let report = switch_report(
        "import 'missing.dart'; int g(U? u) { switch (u) { case var v?: return 1; case null: return 0; } }",
        "class U {}",
        false,
    );
    assert!(family_codes(&report).is_empty(), "{:?}", family_codes(&report));
}

/// The same nullable-scrutinee shape with the import intact: the promoted
/// non-null half makes the `null` arm unreachable exactly as the oracle
/// reports it — the gate keeps the row (the head resolves).
#[test]
fn imported_nullable_scrutinee_keeps_the_unreachable_row() {
    let report = switch_report(
        "import 'kit.dart'; int g(U? u) { switch (u) { case var v?: return 1; case null: return 0; } }",
        "class U {}",
        true,
    );
    assert_eq!(
        family_codes(&report),
        ["unreachable_switch_case"],
        "{:?}",
        family_codes(&report)
    );
}

/// The expression face over an unimported enum scrutinee suppresses the
/// same way (the covering report shares the gate).
#[test]
fn unimported_enum_scrutinee_suppresses_the_expression_face() {
    let report = switch_report(
        "import 'missing.dart'; int f(K k) => switch (k) { K.a => 1, K.b => 2 };",
        "enum K { a, b }",
        false,
    );
    assert!(family_codes(&report).is_empty(), "{:?}", family_codes(&report));
}

/// A resolved enum scrutinee with a missing constant on the expression
/// face keeps the covering row (the must-be-exhaustive face is untouched
/// by the gate).
#[test]
fn imported_enum_missing_constant_keeps_the_expression_face() {
    let report = switch_report(
        "import 'kit.dart'; int f(K k) => switch (k) { K.a => 1 };",
        "enum K { a, b }",
        true,
    );
    assert_eq!(
        family_codes(&report),
        ["non_exhaustive_switch_expression"],
        "{:?}",
        family_codes(&report)
    );
}

/// An ordinary (non-sealed, non-enum) class scrutinee never joins the
/// exhaustiveness verdict: open or not, the family stays empty — the gate
/// leaves the pre-existing posture byte-identical.
#[test]
fn ordinary_class_scrutinee_keeps_the_silent_posture() {
    let report = switch_report(
        "import 'kit.dart'; int f(C c) => switch (c) { C() => 1 };",
        "class C {}",
        true,
    );
    assert!(family_codes(&report).is_empty(), "{:?}", family_codes(&report));
}
