//! #3885 pins — the mixin-family conformance faces.
//!
//! (CUM-1) `class_used_as_mixin_declares_generative_constructor` — a `with`
//! entry resolving to a plain class that declares a generative constructor
//! reports in the else-branch (pre-3.0 library or mayIgnore exempt,
//! upstream `error_verifier.dart:3450` else-branch of the
//! `_checkForAllMixinErrorCodes` arm).
//!
//! (CUM-2) The enclosing container's `extends`/`implements` disallowed
//! heads suppress the whole `with` loop (`_checkClassInheritance`'s
//! short-circuit, error_verifier.dart:2338-2340).
//!
//! (CUM-3) `mixin class X = Y with Z` keeps `is_mixin_class` — the alias
//! declaration is a legal mixin.
//!
//! Widening: `mixin_inherits_from_not_object`,
//! `mixin_class_declares_non_trivial_generative_constructor`,
//! `mixin_class_declaration_extends_not_object`,
//! `mixin_class_declaration_with_clause`, and
//! `mixin_modifier_mixin_application_class_with_multiple_mixins` — the
//! declaration-side `_checkForMixinClassErrorCodes` family.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — every fixture probed there.

use super::test_rig::{report_ops, rows};
use super::{StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::collector::collect;
use crate::diagnose::FileReport;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use crate::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The `dart:core` names-only stub — the same spelling set test_rig mounts.
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// [`report_ops`] with the analyzed library's `class-modifiers` posture
/// pinned off — the pre-3.0 library the `classUsedAsMixin` arm stands down
/// on (the else-branch arms — generative-constructor and
/// inherits-from-not-object — run only when that arm does not). The shared
/// rig always mounts the modern posture, so the pre-3.0 pins carry their
/// own build.
fn report_ops_no_class_modifiers(source: &str) -> FileReport {
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
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
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
    crate::capture_clause_trees(
        file,
        source,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types =
        LibraryTypes::build_with_clauses(&graph, &namespaces, &index, &strings, lib, &clauses);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let mut class_modifiers = LibraryClassModifiers::default();
    class_modifiers.insert(lib, false);
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
        &class_modifiers,
        false,
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn mixin_of_non_class(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "mixin_of_non_class")
        .map(|d| d.message.to_string())
        .collect()
}

fn class_used_as_mixin(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "class_used_as_mixin")
        .map(|d| d.message.to_string())
        .collect()
}

fn class_used_as_mixin_declares_generative_constructor(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "class_used_as_mixin_declares_generative_constructor")
        .map(|d| d.message.to_string())
        .collect()
}

fn mixin_inherits_from_not_object(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "mixin_inherits_from_not_object")
        .map(|d| d.message.to_string())
        .collect()
}

fn mixin_class_declares_non_trivial_generative_constructor(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "mixin_class_declares_non_trivial_generative_constructor")
        .map(|d| d.message.to_string())
        .collect()
}

fn mixin_class_declaration_extends_not_object(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "mixin_class_declaration_extends_not_object")
        .map(|d| d.message.to_string())
        .collect()
}

fn mixin_class_declaration_with_clause(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "mixin_class_declaration_with_clause")
        .map(|d| d.message.to_string())
        .collect()
}

fn mixin_modifier_mixin_application_class_with_multiple_mixins(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "mixin_modifier_mixin_application_class_with_multiple_mixins")
        .map(|d| d.message.to_string())
        .collect()
}

fn subtype_of_disallowed_type(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "subtype_of_disallowed_type")
        .map(|d| d.message.to_string())
        .collect()
}

// —————————————————————————————————————————————————————————————————————————
// CUM-1: `class_used_as_mixin_declares_generative_constructor` (40 missing)
// —————————————————————————————————————————————————————————————————————————

/// A plain class with a generative constructor in a pre-3.0 library reports
/// the `class_used_as_mixin_declares_generative_constructor` face — the
/// else-branch arm of `_checkForAllMixinErrorCodes` fires when
/// `classUsedAsMixin` does not (no `class_modifiers` feature).
#[test]
fn cum1_generative_ctor_in_pre_3_library() {
    let report = report_ops_no_class_modifiers(
        "class A { A(int x); }\n\
         class C = Object with A;\n",
    );
    assert_eq!(
        class_used_as_mixin_declares_generative_constructor(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A plain class WITHOUT a generative constructor in a pre-3.0 library is a
/// legal mixin — no `class_used_as_mixin_declares_generative_constructor`.
#[test]
fn cum1_no_ctor_stays_silent() {
    let report = report_ops_no_class_modifiers(
        "class A {}\n\
         class C = Object with A;\n",
    );
    assert!(
        class_used_as_mixin_declares_generative_constructor(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A mixin declaration in a pre-3.0 library is a legal mixin — no
/// `class_used_as_mixin_declares_generative_constructor` and no
/// `class_used_as_mixin`.
#[test]
fn cum1_mixin_decl_stays_silent() {
    let report = report_ops_no_class_modifiers(
        "mixin A {}\n\
         class C = Object with A;\n",
    );
    assert!(
        class_used_as_mixin_declares_generative_constructor(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// Control: a plain class used as a mixin in a MODERN library still
/// reports `class_used_as_mixin` (the arm fires, the else-branch doesn't).
#[test]
fn cum1_control_class_used_as_mixin_fires() {
    let report = report_ops(
        "class A {}\n\
         class C = Object with A;\n",
    );
    assert_eq!(
        class_used_as_mixin(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin_declares_generative_constructor(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

// —————————————————————————————————————————————————————————————————————————
// CUM-2: enclosing-container suppression (extends/implements disallowed)
// —————————————————————————————————————————————————————————————————————————

/// `class C extends Null with A` suppresses the `with` mixin loop — the
/// disallowed `extends` head short-circuits the whole
/// `_checkForAllMixinErrorCodes` (upstream error_verifier.dart:2338-2340).
#[test]
fn cum2_extends_disallowed_suppresses_with() {
    let report = report_ops(
        "class A {}\n\
         class C extends Null with A {}\n",
    );
    assert!(
        subtype_of_disallowed_type(&report)
            .iter()
            .any(|m| m.contains("Null")),
        "expected subtype_of_disallowed_type on 'Null': {:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
    assert!(
        mixin_inherits_from_not_object(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// `class C with A implements Null` suppresses the `with` mixin loop — the
/// disallowed `implements` head short-circuits the whole
/// `_checkForAllMixinErrorCodes` (the implements check runs before the
/// mixin loop in `_checkClassInheritance`'s `&&` chain).
#[test]
fn cum2_implements_disallowed_suppresses_with() {
    let report = report_ops(
        "class A {}\n\
         class C with A implements Null {}\n",
    );
    assert!(
        subtype_of_disallowed_type(&report)
            .iter()
            .any(|m| m.contains("Null")),
        "expected subtype_of_disallowed_type on 'Null': {:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// `class C extends Null with Record` — the disallowed `extends` head
/// suppresses the `finalize_mixin_uses` family on the `with` entry: no
/// `class_used_as_mixin`, no generative-constructor row, no
/// inherits-from-not-object row. (`Record`'s own clause-miss rows —
/// `subtype_of_disallowed_type` / `mixin_of_non_class` — ride a separate
/// emit site and are out of this pin's scope.)
#[test]
fn cum2_extends_disallowed_whole_with_loop_skipped() {
    let report = report_ops("class C extends Null with Record {}\n");
    assert!(
        subtype_of_disallowed_type(&report)
            .iter()
            .any(|m| m.contains("Null")),
        "expected subtype_of_disallowed_type on 'Null': {:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin_declares_generative_constructor(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
    assert!(
        mixin_inherits_from_not_object(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// `class C extends A with Record` — the `Record` `with` entry is a
/// disallowed SDK type. It reports `subtype_of_disallowed_type` on
/// `Record`, never `class_used_as_mixin` — the disallowed arm precedes the
/// mixin arm inside `_checkForAllMixinErrorCodes`.
#[test]
fn cum2_with_record_reports_disallowed_not_mixin() {
    let report = report_ops(
        "class A {}\n\
         class C extends A with Record {}\n",
    );
    assert!(
        subtype_of_disallowed_type(&report)
            .iter()
            .any(|m| m.contains("Record")),
        "expected subtype_of_disallowed_type on 'Record': {:#?}",
        rows(&report)
    );
    assert!(
        class_used_as_mixin(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

// —————————————————————————————————————————————————————————————————————————
// CUM-3: `mixin class X = Y with Z` keeps `is_mixin_class` (46 extras)
// —————————————————————————————————————————————————————————————————————————

/// `mixin class X = Object with M` is a legal mixin application — the
/// `mixin` keyword marks the alias a mixin class, so `with X` stays silent.
#[test]
fn cum3_mixin_class_alias_is_legal_mixin() {
    let report = report_ops(
        "mixin M {}\n\
         mixin class X = Object with M;\n\
         class C = Object with X;\n",
    );
    assert!(
        class_used_as_mixin(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
    assert!(
        mixin_inherits_from_not_object(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// Control: `class X = Object with M` (no `mixin` keyword) is NOT a legal
/// mixin — `with X` still reports `class_used_as_mixin`.
#[test]
fn cum3_control_class_alias_still_reports() {
    let report = report_ops(
        "mixin M {}\n\
         class X = Object with M;\n\
         class C = Object with X;\n",
    );
    assert_eq!(
        class_used_as_mixin(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

// —————————————————————————————————————————————————————————————————————————
// Widening: `mixin_inherits_from_not_object` (82 missing)
// —————————————————————————————————————————————————————————————————————————

/// A plain class extending non-Object in a PRE-3.0 library reports
/// `mixin_inherits_from_not_object` on the `with` entry — the else-branch
/// arm's second check (modern libraries report `class_used_as_mixin`
/// instead — the `else` never runs for them).
#[test]
fn widening_inherits_from_not_object() {
    let report = report_ops_no_class_modifiers(
        "class A {}\n\
         class B extends A {}\n\
         class C = Object with B;\n",
    );
    assert_eq!(
        mixin_inherits_from_not_object(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

/// `mixin class B extends A` used as `with B` reports
/// `mixin_inherits_from_not_object` — mixin classes reach the else-branch
/// (the `classUsedAsMixin` arm skips them), where the inherits check still
/// fires.
#[test]
fn widening_mixin_class_extends_not_object_use_site() {
    let report = report_ops(
        "class A {}\n\
         mixin class B extends A {}\n\
         class C = Object with B;\n",
    );
    assert_eq!(
        mixin_inherits_from_not_object(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

/// A mixin application `class B = Object with M` used as `with B` in a
/// pre-3.0 library does NOT report `mixin_inherits_from_not_object` — a
/// single-entry mixin application is exempt (upstream
/// `isMixinApplication && mixins.length < 2`). The pre-3.0 posture is what
/// reaches the else-branch at all: a modern library reports
/// `class_used_as_mixin` on `B` instead.
#[test]
fn widening_mixin_application_single_with_exempt() {
    let report = report_ops_no_class_modifiers(
        "mixin M {}\n\
         class B = Object with M;\n\
         class C = Object with B;\n",
    );
    assert!(
        mixin_inherits_from_not_object(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A mixin application `class B = Object with M1, M2` used as `with B` in a
/// pre-3.0 library reports `mixin_inherits_from_not_object` — the
/// application exemption does not cover a 2+ `with` list.
#[test]
fn widening_mixin_application_multi_with() {
    let report = report_ops_no_class_modifiers(
        "mixin M1 {}\n\
         mixin M2 {}\n\
         class B = Object with M1, M2;\n\
         class C = Object with B;\n",
    );
    assert_eq!(
        mixin_inherits_from_not_object(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

// —————————————————————————————————————————————————————————————————————————
// Widening: `mixin_class_declares_non_trivial_generative_constructor`
// (97 missing)
// —————————————————————————————————————————————————————————————————————————

/// A `mixin class` with a declared generative constructor reports
/// `mixin_class_declares_non_trivial_generative_constructor`.
#[test]
fn widening_mixin_class_nontrivial_ctor() {
    let report = report_ops("mixin class B { B(int x); }\n");
    assert_eq!(
        mixin_class_declares_non_trivial_generative_constructor(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

/// A `mixin class` with a trivial (no-param, empty-body) constructor does
/// NOT report — the ctor is `isTrivial` upstream.
#[test]
fn widening_mixin_class_trivial_ctor_silent() {
    let report = report_ops("mixin class B { B(); }\n");
    assert!(
        mixin_class_declares_non_trivial_generative_constructor(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A `mixin class` with NO declared constructor stays silent — the
/// implicit default is not a declaration.
#[test]
fn widening_mixin_class_no_ctor_silent() {
    let report = report_ops("mixin class B {}\n");
    assert!(
        mixin_class_declares_non_trivial_generative_constructor(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

// —————————————————————————————————————————————————————————————————————————
// Widening: `mixin_class_declaration_extends_not_object` (53 missing)
// —————————————————————————————————————————————————————————————————————————

/// A `mixin class` declaring `extends` non-Object reports
/// `mixin_class_declaration_extends_not_object`.
#[test]
fn widening_mixin_class_extends_not_object() {
    let report = report_ops(
        "class A {}\n\
         mixin class B extends A {}\n",
    );
    assert_eq!(
        mixin_class_declaration_extends_not_object(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

/// A `mixin class` extending `Object` stays silent — `Object` is legal.
#[test]
fn widening_mixin_class_extends_object_silent() {
    let report = report_ops("mixin class B extends Object {}\n");
    assert!(
        mixin_class_declaration_extends_not_object(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

// —————————————————————————————————————————————————————————————————————————
// Widening: `mixin_class_declaration_with_clause` (27 missing)
// —————————————————————————————————————————————————————————————————————————

/// A `mixin class` body declaration carrying `with` reports
/// `mixin_class_declaration_with_clause` — a non-application cannot use
/// `with`.
#[test]
fn widening_mixin_class_with_clause() {
    let report = report_ops(
        "mixin M {}\n\
         mixin class B with M {}\n",
    );
    assert_eq!(
        mixin_class_declaration_with_clause(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

/// A `mixin class` WITHOUT `with` stays silent — the body declaration is
/// legal.
#[test]
fn widening_mixin_class_no_with_silent() {
    let report = report_ops("mixin class B {}\n");
    assert!(
        mixin_class_declaration_with_clause(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// A `mixin class X = S with M` alias declaration does NOT report
/// `mixin_class_declaration_with_clause` — the `= ` form IS a mixin
/// application, not a body declaration with `with`.
#[test]
fn widening_mixin_class_alias_with_clause_silent() {
    let report = report_ops(
        "mixin M {}\n\
         mixin class X = Object with M;\n",
    );
    assert!(
        mixin_class_declaration_with_clause(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

// —————————————————————————————————————————————————————————————————————————
// Widening: `mixin_modifier_mixin_application_class_with_multiple_mixins`
// (10 missing)
// —————————————————————————————————————————————————————————————————————————

/// `mixin class X = Object with M1, M2` — a mixin application with 2+
/// `with` entries reports the multiple-mixins face.
#[test]
fn widening_mixin_application_multiple_mixins() {
    let report = report_ops(
        "mixin M1 {}\n\
         mixin M2 {}\n\
         mixin class X = Object with M1, M2;\n",
    );
    assert_eq!(
        mixin_modifier_mixin_application_class_with_multiple_mixins(&report).len(),
        1,
        "{:#?}",
        rows(&report)
    );
}

/// `mixin class X = Object with M` — a single-entry mixin application
/// stays silent.
#[test]
fn widening_mixin_application_single_silent() {
    let report = report_ops(
        "mixin M {}\n\
         mixin class X = Object with M;\n",
    );
    assert!(
        mixin_modifier_mixin_application_class_with_multiple_mixins(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}
