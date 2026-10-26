//! The #2462 pins: the is-check fold over argument-carrying types (census
//! family A5 — the drift/dio rows where dartq folded
//! `stream is Stream<Uint8List>` on a `Stream<List<int>>` receiver
//! constant-true while the oracle keeps the test live).
//!
//! The mechanism: the name-level projection dropped the written type
//! arguments, so two instantiations of one head compared equal and the
//! constant-true fold fired on the collapse. The projection now carries
//! the argument slot ([`crate::switch_tracker::NameTy::NamedArgs`]) and
//! the argument-carrying pair re-asks the semantic subtype face — only a
//! proven subtype folds; a decidable miss and an unprovable pair both
//! decline and the test stays live (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/subtype.dart, the interface type
//! case: same declaration answers through the type argument list, distinct
//! heads through the supertype graph).
//!
//! Oracle provenance: the `/tmp/probe2462` rigs against `dart analyze` —
//! `Stream<int> is Stream<int>`, `List<int> is List<num>`, and
//! `List<int> is Iterable<num>` fold constant-true;
//! `Stream<List<int>> is Stream<Uint8List>`, `List<int> is
//! Iterable<String>`, and `List<T extends num> is List<int>` stay live
//! with no `dead_code`.

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the engine path the is-check folds decide
/// through (the #2450 harness shape).
fn report_ops(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
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
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
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
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— (1) the census shape: differing arguments keep the test live ——————

/// `Box<List<int>> is Box<Uint8List>` on a `Box<List<int>>` receiver: the
/// argument lists decide the pair and `List<int>` does not fit
/// `Uint8List`, so the fold declines — no `unnecessary_type_check`, and
/// the flow keeps both arms live (the drift rows' rig translation).
#[test]
fn differing_type_arguments_keep_the_is_check_live() {
    let source = "\
class num {}
class int extends num {}
class String {}
class List<E> {}
class Box<T> {
  Box();
}

class Uint8List {
  Uint8List();
}

void f(Box<List<int>> stream) {
  if (stream is Box<Uint8List>) {
    stream;
  } else {
    stream;
  }
}";
    let codes = code_names(&report_ops(source));
    assert!(
        !codes.contains(&"unnecessary_type_check"),
        "the fold must decline over differing arguments: {codes:?}"
    );
    assert!(
        !codes.contains(&"dead_code"),
        "both arms stay live when the fold declines: {codes:?}"
    );
}

// —— (2) the provable same-type fold stays ————————————————————————————

/// `Box<int> is Box<int>`: the identical spelling is still the provable
/// same-type fold (the oracle reports the check always-true — the
/// kept true positive).
#[test]
fn same_arguments_stay_constant_true() {
    let source = "\
class num {}
class int extends num {}
class String {}
class List<E> {}
class Box<T> {
  Box();
}

void f(Box<int> b) {
  if (b is Box<int>) {
    b;
  }
}";
    let codes = code_names(&report_ops(source));
    assert_eq!(codes, ["unnecessary_type_check"], "{codes:?}");
}

// —— (3) the provable covariant fold stays —————————————————————————————

/// `Box<int> is Box<num>`: the covariant argument fit (`int <: num`, the
/// interface rule's pairwise ask) still folds constant-true — the oracle
/// reports it (probe2462/edges line 2) and the decline must not overfire.
#[test]
fn covariant_arguments_stay_constant_true() {
    let source = "\
class num {}
class int extends num {}
class String {}
class List<E> {}
class Box<T> {
  Box();
}

void f(Box<int> b) {
  if (b is Box<num>) {
    b;
  }
}";
    let codes = code_names(&report_ops(source));
    assert_eq!(codes, ["unnecessary_type_check"], "{codes:?}");
}

// —— (4) the type-variable tested face stays conservative ——————————————

/// `Box<T> is Box<String>` over an unbounded `T`: `T <: String` is
/// unprovable at this layer, so the fold declines and the test stays live
/// (the oracle keeps it live too — its boundless-parameter rule).
#[test]
fn type_variable_tested_face_stays_conservative() {
    let source = "\
class num {}
class int extends num {}
class String {}
class List<E> {}
class Box<T> {
  Box();
}

void f<T>(Box<T> b) {
  if (b is Box<String>) {
    b;
  } else {
    b;
  }
}";
    let codes = code_names(&report_ops(source));
    assert_eq!(codes, Vec::<&'static str>::new(), "{codes:?}");
}

// —— (5) flow innocence on the declined fold ———————————————————————————

/// The declined fold leaves the promoted/demoted arms working: the
/// if-case's then arm narrows, the else arm stays reachable, and no
/// `dead_code` lands on either (the flow face runs its own verdict — the
/// semantic decline must not deaden the branches the oracle keeps).
#[test]
fn declined_fold_keeps_the_demoted_arm_live() {
    let source = "\
class num {}
class int extends num {}
class String {}
class List<E> {}
class Box<T> {
  Box();
}

class Uint8List {
  Uint8List();
}

String g(Box<List<int>> stream) {
  if (stream is Box<Uint8List>) {
    return 'typed';
  }
  return 'plain';
}

String h(Box<List<int>> stream) {
  if (stream is! Box<Uint8List>) {
    return 'plain';
  }
  return 'typed';
}";
    let codes = code_names(&report_ops(source));
    assert_eq!(codes, Vec::<&'static str>::new(), "{codes:?}");
}

// —— (6) the nullability gate keeps its verdict ————————————————————————

/// `Box<int>? is Box<int>`: the nullable receiver against the bare target
/// never proved a constant (null fits no plain `Box`), and the
/// argument-carrying projection keeps that gate ahead of the argument ask.
#[test]
fn nullable_receiver_against_bare_target_stays_live() {
    let source = "\
class num {}
class int extends num {}
class String {}
class List<E> {}
class Box<T> {
  Box();
}

void f(Box<int>? b) {
  if (b is Box<int>) {
    b;
  }
}";
    let codes = code_names(&report_ops(source));
    assert_eq!(codes, Vec::<&'static str>::new(), "{codes:?}");
}

// —— (7) the landed #2450 neighborhood is untouched ————————————————————
// The undefined tested head folds nowhere (`type_test_with_undefined_name`
// on the spelling, both arms live) — pinned by
// `ticket_2450_tests::invalid_operand_undefined_tested_keeps_both_arms`
// and the dead-family suite, both re-run green alongside these pins; the
// argument-aware arm only decides pairs whose sides both resolve.
