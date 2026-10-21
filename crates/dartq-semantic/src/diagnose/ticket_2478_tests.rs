//! The #2478 pins.
//!
//! G5c (census riverpod async_value.dart 741:29): a record literal in a
//! const constructor's initializer list is not `isConst` —
//! `constantContext2` answers null there — so the
//! `_ConstLiteralVerifier` element faces never run on it; the
//! initializer's gate is the weaker potentially-constant scan
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart
//! `_validateConstructorInitializers` +
//! pkg/analyzer/lib/src/dart/constant/potentially_constant.dart
//! `_nameExpression`), under which a formal-parameter reference of the
//! enclosing const constructor is potentially constant. dartq's element
//! check evaluated the field with the plain engine lookup, whose
//! in-scope-formal verdict faulted the reference into a bogus
//! `non_constant_record_field`; the element verdicts now evaluate with
//! the constructor's formals bound as potentially-constant names, and a
//! genuinely non-constant field in a true const context still reports.
//!
//! G10 (census petitparser position.dart 22:51, predicate.dart 57:53,
//! 63:7) keeps one honesty pin here: a GENUINE discarded
//! `@useResult`-annotated top-level invocation must keep reporting
//! `unused_result`. The shadowing misfire itself (a parameter or field
//! merely sharing the annotated function's name) is fixed in the
//! discarded-value verifier owned by another lane — the pin stands so
//! that fix cannot silence the true positive.

use super::*;
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
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

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports (the core containers the shapes name are
/// declared in-source — the unit world carries no dart:core).
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
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
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
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
    )
}

/// The meta-import rig: a `library meta;` dependency declares the
/// `useResult` getter and is imported unfiltered, so the annotation's
/// meta provenance resolves (the corpus's package:meta posture).
fn meta_report(main_source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &["Object", "String", "int", "bool", "double", "num", "Match"],
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    let dep_source = "library meta;\nbool get useResult => true;\n";
    let dep_file = files.intern(Path::new("meta.dart"));
    let dep_lib = builder.intern("package:t/meta.dart");
    builder.library_name(dep_lib, "meta");
    let scanned_dep = dartq_scanner::scan(
        dep_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dep_file,
    );
    assert!(scanned_dep.diagnostics.is_empty());
    let dep_elements = collect(dep_source, &scanned_dep.tokens, features, &strings);
    for (name, decl) in dep_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(dep_lib, name, dep_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(dep_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
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

fn codes(report: &FileReport, name: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The exact riverpod `async_value.dart` 741 shape: the const
/// constructor's record initializer references the formal parameter —
/// potentially constant, so the oracle is silent (probe2478 a1: `No
/// issues`) and the engine must be too. The name-resolution layer
/// classifies the in-scope formal `NotConstant`; the element check runs
/// inside the const constructor's initializer list, where the formals
/// bind as potentially-constant names instead.
#[test]
fn const_record_formal_field_stays_silent() {
    let source = "\
class num {}

class L {
  final ({num? progress}) rec;
  const L({num? progress}) : rec = (progress: progress);
}
";
    let report = unit_report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The initializing formal rides the same pass: its reference spelling
/// joins the constructor's formal table, so `rec = (v: q)` stays silent
/// beside the plain-formal shape (probe2478 a4: silent — a4's compound
/// arithmetic variant `p + 1` is equally silent there but rides core
/// operator faces the unit world cannot model).
#[test]
fn initializing_formal_record_field_stays_silent() {
    let source = "\
class int {}

class K {
  final ({int? v}) rec;
  final int q;
  const K({int? p, required this.q}) : rec = (v: q);
}
";
    let report = unit_report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The honesty face: a record field referencing a genuinely non-constant
/// expression in a true const context (a `const` declaration
/// initializer — outside any constructor initializer list, where the
/// element face is the oracle's own) still reports
/// `non_constant_record_field` anchored at the failing leaf (probe2478
/// a3: `error 16:17 non_constant_record_field`, the local).
#[test]
fn non_const_record_field_in_const_declaration_still_fires() {
    let source = "\
class num {}

void m() {
  num l = 1;
  const a = (x: l);
}
";
    let report = unit_report(source);
    let at = (source.find("l);").unwrap() as u32);
    assert_eq!(
        codes(&report, "non_constant_record_field"),
        vec![(at, 1)],
        "{:?}",
        report.diagnostics
    );
}

/// The G10 honesty pin: a genuine discarded `@useResult`-annotated
/// top-level invocation keeps reporting `unused_result` at the callee
/// name token (probe2478 b: `warning 7:3 unused_result`, the sole row).
/// The shadowing fix in the discarded-value verifier must keep this true
/// positive green.
#[test]
fn genuine_unused_use_result_still_fires() {
    let source = "\
import 'meta.dart';

@useResult
int predicate(int x) => x;

void fire() {
  predicate(1);
}
";
    let report = meta_report(source);
    let at = source.rfind("predicate").unwrap() as u32;
    assert_eq!(
        codes(&report, "unused_result"),
        vec![(at, "predicate".len() as u32)],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
}
