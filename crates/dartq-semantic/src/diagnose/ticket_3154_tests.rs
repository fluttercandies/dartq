//! The #3154 `subtype_of_disallowed_type` pins: the clause-face emission
//! after the `disallowed_subtype_head` gate was re-keyed on the RESOLVED
//! element's library — `dart:*` owner = restricted SDK face (fires), user
//! owner = shadow (silent), unresolved = closed-spelling face (fires) —
//! and the row's anchor moved from the declaring container's name onto
//! each written clause `NamedType` (upstream's
//! `diagnosticCode.withArguments(disallowedType: type).at(namedType)`,
//! inheritance_override.dart:646-651).
//!
//! The fixtures ride a names-only `dart:core` stub (the names-only SDK
//! projection grade) plus an optional user-decl prelude exercising the
//! shadow arm. This file is registered on `crate::bounds` — outside
//! `crate::diagnose`'s private `test_rig` module — so it mounts the same
//! engine face (`fold_unit_with_ops`) through the crate's public entries.

use crate::clause_capture::capture_clause_trees;
use crate::collector::collect;
use crate::diagnose::types::FileReport;
use crate::diagnose::{StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use crate::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The member-bearing core classes a fixture may name — the rig's user
/// declarations ride the SAME spellings the disallowed table restricts, so
/// a fixture referencing `Null`/`Record`/`FutureOr` through this prelude
/// resolves the element onto the USER face and exercises the shadow arm.
const CORE_PRELUDE: &str = "class num {} \
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; bool operator >(int o) => true; bool operator <(int o) => true; } \
     class double extends num {} \
     class String { String operator +(String other) => this; int get length => 0; } class bool {} class Iterable<E> {} \
     class List<E> extends Iterable<E> { void add(E e) {} E operator [](int i) => throw 0; int get length => 0; } \
     class Map<K, V> { V? operator [](K k) => null; Iterable<K> get keys => throw 0; } \
     class Set<E> extends Iterable<E> {} class Exception {} \
     abstract class Enum { int get index => 0; }\n";

/// The names-only `dart:core` stub: the special spellings the disallowed
/// table restricts resolve as types through an unindexed file — the
/// element lands on the `dart:core` library, so the `is_sdk` arm fires.
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// Diagnoses `source` with the resolved-type snapshot mounted and no
/// prelude — every name the fixture spells resolves either through the
/// `dart:core` stub (SDK face) or misses (closed-spelling face).
fn report_ops_bare(source: &str) -> FileReport {
    report_ops_with(source)
}

/// [`report_ops_bare`] with [`CORE_PRELUDE`] declared ahead of the
/// fixture — the prelude's user `Null`/`Record`/`FutureOr`/`Enum`/`String`
/// declarations shadow the SDK spellings.
fn report_ops(source: &str) -> FileReport {
    report_ops_with(&format!("{CORE_PRELUDE}{source}"))
}

/// The single-library rig: scan -> collect -> one `package:t` library
/// importing `dart:core` (names-only stub) -> clause trees -> resolved ops
/// -> `fold_unit_with_ops`. Mirrors `crate::diagnose::test_rig`'s mount so
/// the clause-face teardown sees the same graph shape it sees there.
fn report_ops_with(source: &str) -> FileReport {
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
            non_library: false,
            combinators: Combinators::unfiltered(),
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
            builder.declare_class(lib, name, file, ClassModifiers::default());
        } else {
            builder.declare(lib, name, file);
        }
    }
    let mut clauses = ClauseTrees::empty();
    capture_clause_trees(
        file,
        source,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, lib, &clauses);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
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
        set.is_enabled(Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// The `subtype_of_disallowed_type` rows of `report` as `message @offset`
/// strings — the pins assert these exactly, ignoring any neighbour face a
/// fixture may carry.
fn subtype_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "subtype_of_disallowed_type")
        .map(|d| format!("{} @{}", d.message, d.range.offset))
        .collect()
}

/// `Enum` never reaches `isNonSubtypableClass` upstream — the
/// `isDartCoreEnum` arm in `_checkDirectSuperType` intercepts every clause
/// face (inheritance_override.dart:610-620). The pin sweeps all four faces.
#[test]
fn enum_faces_never_fire_subtype_of_disallowed_type() {
    for src in [
        "class A extends Enum {}",
        "class A with Enum {}",
        "class A implements Enum {}",
        "mixin M on Enum {}",
        "mixin M implements Enum {}",
    ] {
        let report = report_ops_bare(src);
        assert!(
            subtype_rows(&report).is_empty(),
            "{src:?}: {:?}",
            subtype_rows(&report)
        );
    }
}

/// A `dart:*`-resolved restricted spelling reports once per clause, with
/// the row anchored at the WRITTEN `NamedType` — never the declaring
/// container's name (the oracle's `.at(namedType)` contract,
/// inheritance_override.dart:648-649).
#[test]
fn sdk_resolved_restricted_spelling_fires_per_clause() {
    for (src, msg) in [
        (
            "class A extends Null {}",
            "Classes can't extend 'Null'. @16",
        ),
        (
            "class A with Record {}",
            "Classes can't mixin 'Record'. @13",
        ),
        (
            "class A implements Null {}",
            "Classes and mixins can't implement 'Null'. @19",
        ),
        (
            "mixin M on Null {}",
            "'Null' can't be used as a superclass constraint. @11",
        ),
        (
            "mixin M implements Record {}",
            "Classes and mixins can't implement 'Record'. @19",
        ),
    ] {
        let report = report_ops_bare(src);
        assert_eq!(subtype_rows(&report), [msg], "{src:?}");
    }
}

/// The bare-alias chase (`typedef T = Null`) resolves the aliased head to
/// the SDK face — the report keeps the WRITTEN alias name AND its span
/// (`Classes can't mixin 'T'.`, messages.yaml :17358 — `{0}` is the
/// written type, `.at(namedType)`).
#[test]
fn bare_alias_to_sdk_face_reports_written_name() {
    let report = report_ops_bare("typedef T = Null;\nclass A with T {}");
    assert_eq!(
        subtype_rows(&report),
        ["Classes can't mixin 'T'. @31"],
        "{:?}",
        subtype_rows(&report)
    );
}

/// A user declaration under a restricted spelling shadows the SDK face —
/// `isNonSubtypableClass` keys on the resolved element's library, so a
/// user-defined `int`/`Null`/`FutureOr` stays silent.
#[test]
fn user_shadow_silences_every_clause_face() {
    for (decl, uses) in [
        (
            "class Null {}",
            &[
                "class A extends Null {}",
                "class A implements Null {}",
                "mixin M on Null {}",
            ][..],
        ),
        (
            "class Record {}",
            &[
                "class A with Record {}",
                "class A implements Record {}",
                "mixin M implements Record {}",
            ][..],
        ),
        (
            "class FutureOr {}",
            &[
                "class A extends FutureOr {}",
                "class A implements FutureOr {}",
            ][..],
        ),
    ] {
        for tail in uses {
            let report = report_ops(&format!("{decl}\n{tail}"));
            assert!(
                subtype_rows(&report).is_empty(),
                "{decl} + {tail:?}: {:?}",
                subtype_rows(&report)
            );
        }
    }
}

/// A class-type-alias `with` face reports through the `mixin` clause —
/// `class A = Object with Null;` carries its `with` list into the
/// `mixinOfDisallowedClass` arm (inheritance_override.dart:684-692), and
/// the row anchors the `Null` entry's written span.
#[test]
fn class_type_alias_with_face_reports_mixin_message() {
    let report = report_ops_bare("class A = Object with Null;");
    assert_eq!(
        subtype_rows(&report),
        ["Classes can't mixin 'Null'. @22"],
        "{:?}",
        subtype_rows(&report)
    );
}

/// The enum container's `with`/`implements` faces both report — the
/// `_checkDirectSuperTypes` walk visits `withClause` and `implementsClause`
/// for enum classElements (inheritance_override.dart:657-697), each row
/// anchored at its own written entry.
#[test]
fn enum_container_faces_report_per_clause() {
    let report = report_ops_bare("enum E with Null implements Record { v }");
    let rows = subtype_rows(&report);
    assert_eq!(
        rows,
        [
            "Classes can't mixin 'Null'. @12",
            "Classes and mixins can't implement 'Record'. @28",
        ],
        "{rows:?}"
    );
}

/// The mixin-class declaration's `extends`/`implements` faces report — a
/// `mixin class` element reaches `_checkDirectSuperTypes` like a class
/// (inheritance_override.dart:679-693), each row at its own entry.
#[test]
fn mixin_class_faces_report_per_clause() {
    let report = report_ops_bare("mixin class A extends Null implements Record {}");
    let rows = subtype_rows(&report);
    assert_eq!(
        rows,
        [
            "Classes can't extend 'Null'. @22",
            "Classes and mixins can't implement 'Record'. @38",
        ],
        "{rows:?}"
    );
}

/// A clean clause shape stays silent — the disallowed table is not the
/// catch-all: user elements, unresolved non-table spellings, `Function`
/// (not in `_nonSubtypableClassNames`).
#[test]
fn clean_shapes_stay_silent() {
    // `Function` IS an InterfaceType but not in `_nonSubtypableClassNames`.
    let function_extends = report_ops_bare("class A extends Function {}");
    assert!(
        subtype_rows(&function_extends).is_empty(),
        "{:?}",
        subtype_rows(&function_extends)
    );
    let function_on = report_ops_bare("mixin M on Function {}");
    assert!(
        subtype_rows(&function_on).is_empty(),
        "{:?}",
        subtype_rows(&function_on)
    );
    // A user element satisfies every clause.
    let clean = report_ops_bare("class B {}\nclass A extends B implements B {}");
    assert!(
        clean.diagnostics.is_empty(),
        "{:?}",
        clean.diagnostics
    );
    // An unresolved non-table spelling keeps the no-SDK-face verdict.
    let unres = report_ops_bare("class A extends UnresolvedType {}");
    assert!(
        subtype_rows(&unres).is_empty(),
        "{:?}",
        subtype_rows(&unres)
    );
}
