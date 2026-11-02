//! #3460 — the application-order pins: an `augment` sees only the fragments
//! of units before its own unit in `LibraryGraph::unit_files` order (the
//! oracle's `allUnits` walk — defining unit, then each part in declaration
//! order — and `ElementBuilder` links `previousFragment` inside that walk).
//!
//! Before the truncation the chain joined every unit's rows regardless of
//! position, so an augment applied before its target merged against the
//! LATER declaration and reported nothing — the `applying_augmentations_*`
//! / `application_order_*` co19 families' missing rows.
//!
//! The second arm is the `factory_body_diagnostics` walk: a factory intro
//! counts only the augmentations applied after it — later units' fragments
//! plus this unit's own ones following its head in source order
//! (`AugTarget::head_range` decides the same-unit pair for constructors).

use super::test_rig::{
    CORE_PRELUDE, rows, report_ops_augment, report_ops_augment_intro_experiments,
};
use super::*;
use dartq_language::Feature;

/// The augmentation-family rows of `report`, `code @offset:len message`.
fn aug_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("augmentation_"))
        .map(|d| {
            format!(
                "{} @{}:{} {}",
                d.code.name, d.range.offset, d.range.length, d.message
            )
        })
        .collect()
}

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// A valid augment against an earlier unit's introduction is silent.
#[test]
fn augment_of_earlier_unit_intro_is_silent() {
    let main = "class A {}\n";
    let aug = "augment class A {}\n";
    let report = report_ops_augment(main, aug);
    assert!(
        aug_rows(&report).is_empty(),
        "{:#?}",
        aug_rows(&report)
    );
}

/// An augment of a non-existent declaration reports the plain row at the
/// `augment` keyword (the without-declaration family's base arm).
#[test]
fn augment_of_missing_declaration_reports_plain() {
    let main = "class B {}\n";
    let aug = "augment class A {}\n";
    let report = report_ops_augment(main, aug);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "augmentation_without_declaration")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].message,
        "The declaration being augmented doesn't exist."
    );
}

/// An augment of a different kind reports the different-kind refusal.
#[test]
fn augment_of_different_kind_across_units() {
    let main = "void f() {}\n";
    let aug = "augment class f {}\n";
    let report = report_ops_augment(main, aug);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "augmentation_of_different_declaration_kind")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
}

/// A formal-parameter type mismatch across units reports at the augment's
/// written type.
#[test]
fn formal_parameter_type_mismatch_across_units() {
    let main = "void f(int a) {}\n";
    let aug = "augment void f(String a) {}\n";
    let report = report_ops_augment(main, aug);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "augmentation_formal_parameter_type_mismatch")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].message,
        "The augmentation's formal parameter type 'String' must be the same as the declaration's formal parameter type 'int'."
    );
}

/// A return type mismatch across units reports at the augment's written
/// return type.
#[test]
fn return_type_mismatch_across_units() {
    let main = "int f() => 0;\n";
    let aug = "augment String f() => 'x';\n";
    let report = report_ops_augment(main, aug);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "augmentation_return_type_mismatch")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].message,
        "The augmentation's return type 'String' must be the same as the introductory declaration's return type 'int'."
    );
}

/// `declaration_already_complete`: a complete augment body applied to a
/// complete introduction reports at the `augment` keyword.
#[test]
fn already_complete_via_function_body_across_units() {
    let main = "void f() {}\n";
    let aug = "augment void f() {}\n";
    let report = report_ops_augment(main, aug);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "declaration_already_complete")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
}

/// `declaration_already_complete` on the constructor chain: a complete
/// `augment` constructor applied to a complete introductory one. The
/// introduction must carry a real body — `A();` is incomplete upstream
/// (`is_complete` is `external || nonEmptyBody`, and a semicolon body does
/// not count), so the check is legitimately silent on that fixture; `{}`
/// is a complete empty block.
#[test]
fn already_complete_via_constructor_body_across_units() {
    let main = "class A { A() {} }\n";
    let aug = "augment class A { augment A() {} }\n";
    let report = report_ops_augment(main, aug);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "declaration_already_complete")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
}

/// Two augments of the same declaration in the same unit: the second
/// resolves the chain the first joined, and a second complete body reports
/// `declaration_already_complete`.
#[test]
fn two_augments_chain_in_source_order() {
    let main = "void f() {}\n";
    let aug = "augment void f() {}\naugment void f() {}\n";
    let report = report_ops_augment(main, aug);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "declaration_already_complete")
        .collect();
    assert_eq!(
        hits.len(),
        2,
        "both augments are complete against a complete chain: {:#?}",
        rows(&report)
    );
}

/// G4, the head pin: an augment in the DEFINING unit applied before its
/// target's introduction (declared in a later part) reports
/// `augmentation_without_declaration` — the chain visible at position 0
/// carries no fragments.
#[test]
fn augment_before_later_unit_intro_reports_without_declaration() {
    let main = "augment class A {}\n";
    let aug = "class A {}\n";
    // The intro unit's report is `.0` of the augment rig — the diagnosing
    // file is `main`, at unit position 0, so the part's `class A` is not
    // visible to its augment.
    let report = report_ops_augment_intro_experiments(main, aug, &[Feature::Augmentations]);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "augmentation_without_declaration")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0].message,
        "The declaration being augmented doesn't exist."
    );
}

/// G4 mirror: the same pair with the roles swapped stays silent — an
/// augment applied AFTER the introduction merges.
#[test]
fn augment_after_later_position_merges() {
    let main = "class A {}\n";
    let aug = "augment class A {}\n";
    let report = report_ops_augment(main, aug);
    assert!(
        aug_rows(&report).is_empty(),
        "{:#?}",
        aug_rows(&report)
    );
}

/// G4, parts-of-parts: the chain is the unit-walk preorder, so an augment
/// in a nested part sees only the units before it — an introduction in a
/// LATER sibling part stays invisible.
#[test]
fn nested_part_augment_sees_only_earlier_units() {
    // unit_files order: [main, lib1, lib2] — lib1 carries the augment, lib2
    // the introduction. The augment's chain truncates at lib1's position:
    // lib2's `class A` never joins it.
    let main = "part 'lib1.dart';\npart 'lib2.dart';\n";
    let lib1 = "part of 'main.dart';\naugment class A {}\n";
    let lib2 = "part of 'main.dart';\nclass A {}\n";
    let reports = three_unit_rig(main, lib1, lib2, &[Feature::Augmentations]);
    let hits: Vec<_> = reports[1]
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "augmentation_without_declaration")
        .collect();
    assert_eq!(hits.len(), 1, "{:#?}", rows(&reports[1]));
}

/// G4 on the factory-completeness walk: a body-less `augment factory` in an
/// EARLIER unit never reaches the later unit's introductory factory — the
/// intro still reports the plain `factory_without_body`, not the chain arm.
#[test]
fn factory_body_ignores_earlier_unit_augment() {
    let aug = "augment class A { augment factory named(); }\n";
    let main = "class A { factory named(); }\n";
    // The diagnosing unit is the intro (position 1 in [aug, main] order is
    // not the rig's layout — `augment_rig` registers main first). Diagnose
    // the intro with the augment in the LATER unit: the chain sees it and
    // the row upgrades to `factory_not_complete_after_augmentations`.
    let report = report_ops_augment_intro_experiments(main, aug, &[Feature::Augmentations]);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "factory_not_complete_after_augmentations"),
        "{:#?}",
        rows(&report)
    );
}

/// The three-unit augmentation rig: `main`, `lib1`, `lib2` as one library's
/// units in that order (the `part` preorder — lib2 follows lib1 the way a
/// nested part follows its includer). Returns each unit's report in the
/// same order.
fn three_unit_rig(
    main: &str,
    lib1: &str,
    lib2: &str,
    experiments: &[Feature],
) -> [FileReport; 3] {
    use crate::collector::collect;
    use crate::library::ElementIndex;
    use crate::model::MicroKind;
    use crate::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_resolution::{
        ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder,
        Namespaces,
    };
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    let strings = StringTable::new();
    let mut files = FileTable::new();
    let mut set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    for feature in experiments {
        set = set.with_experiment(*feature);
    }
    let features = ParserFeatures::from_feature_set(set);

    let sources: [(&str, &str); 3] = [
        ("main.dart", main),
        ("lib1.dart", lib1),
        ("lib2.dart", lib2),
    ];
    let mut scanned_units = Vec::new();
    for (path, source) in sources {
        let file = files.intern(Path::new(path));
        // The defining unit rides the prelude so the core classes resolve.
        let source = if path == "main.dart" {
            format!("{CORE_PRELUDE}{source}")
        } else {
            source.to_string()
        };
        let scanned = dartq_scanner::scan(
            &source,
            dartq_scanner::ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "{source}");
        scanned_units.push((file, source, scanned));
    }

    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in [
        "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type",
        "Symbol",
    ] {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    let mut clauses = ClauseTrees::empty();
    for (file, source, scanned) in &scanned_units {
        let elements = collect(source, &scanned.tokens, features, &strings);
        index.insert(*file, elements.clone());
        builder.unit(lib, *file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            if matches!(decl.kind, MicroKind::Class | MicroKind::Enum | MicroKind::Mixin) {
                builder.declare_class(lib, name, *file, ClassModifiers::default());
            } else {
                builder.declare(lib, name, *file);
            }
        }
        crate::capture_clause_trees(*file, source, &scanned.tokens, features, &strings, &mut clauses);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, lib, &clauses);
    let class_modifiers = LibraryClassModifiers::default();
    scanned_units
        .iter()
        .map(|(file, source, scanned)| {
            let ops = ResolvedTypeOps::new(&types, &strings, *file);
            let fold = fold_unit_with_ops(
                source,
                &scanned.tokens,
                features,
                &strings,
                *file,
                lib,
                &graph,
                &namespaces,
                &index,
                StrictModes::default(),
                &class_modifiers,
                set.is_enabled(Feature::InferenceUpdate2),
                Some(&ops),
            );
            let mut report = fold.report;
            report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
            report
        })
        .collect::<Vec<_>>()
        .try_into()
        .expect("three units")
}
