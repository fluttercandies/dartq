//! The #2485 pins: behavior guards over the hot-path perf refactor (the
//! member-access credit loop's whole-list clone removal and the numeric
//! shadow-guard's per-head verdict memo). The refactor is byte-identical
//! by contract; these pins hold the observable faces it touches so a
//! regression trips here:
//!
//! - the unshadowed core-numeric head still refines over an invalid
//!   right operand across repeated sends on the same head (the memo's
//!   miss-then-hit path answers `double` every time);
//! - a container declaration shadowing a core numeric name (`int` or
//!   `num`) keeps the node at the declared operator return — the guard
//!   fires on the first send and the memoized verdict on any later one;
//! - the extension-use credit adjudication still walks a
//!   many-member-read file identically (the used universal-face member
//!   spares, the unused one reports).

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
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

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn codes(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

fn messages(report: &FileReport, name: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| d.message.to_string())
        .collect()
}

/// The unshadowed hot path: three sends on the same `int` head — the
/// first computes the shadow verdict (no container declares `int`), the
/// next two read the memo — and every node still refines `int <op>
/// invalid` to `double` (the `argument_type_not_assignable` row names
/// 'double' each time).
#[test]
fn unshadowed_int_refines_across_repeated_sends() {
    let source = "\
class Node {
  int end = 0;
}

void g({required num length}) {}

void f(Node node, Token operator) {
  g(length: node.end + operator.offset);
  g(length: node.end - operator.offset);
  g(length: node.end * operator.offset);
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        3,
        "{:?}",
        codes(&report)
    );
    assert!(
        messages(&report, "argument_type_not_assignable")
            .iter()
            .all(|m| m.contains("'double'")),
        "{:?}",
        messages(&report, "argument_type_not_assignable")
    );
}

/// The `int` slot's shadowed face: a container declaration named `int`
/// owns the operator face, the guard blocks the refinement, and the
/// declared operator return (assignable to the `num` parameter) keeps
/// the argument silent — the exact face the memo's `true` verdict must
/// reproduce on every send.
#[test]
fn shadowed_int_keeps_declared_return() {
    let source = "\
class int {}

void g({required num length}) {}

void f() {
  int a = 0;
  g(length: a + missing);
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        0,
        "{:?}",
        codes(&report)
    );
}

/// The `num` slot: the head IS `num` and a container declares it — the
/// `name == num` short-circuit makes the verdict `true` from the single
/// own-container probe, so the node keeps the declared return instead of
/// the `double` refinement.
#[test]
fn shadowed_num_keeps_declared_return() {
    let source = "\
class num {}

void g({required num length}) {}

void f() {
  num a = 0;
  g(length: a + missing);
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        0,
        "{:?}",
        codes(&report)
    );
}

/// The member-read credit loop over a many-read file: the external
/// `mapOrNull` access, the class member reads, and the in-body implicit
/// `this` reads all ride the same pass — the used universal-face member
/// stays spared and the never-referenced sibling reports exactly once.
#[test]
fn many_member_reads_credit_adjudication_unchanged() {
    let source = "\
class N {
  int tag = 0;
}

class K {
  void run() {}
}

extension<T> on T {
  R? mapOrNull<R>(R Function(T) mapper) => mapper(this);
  void visit() {}
}

void f(N x, K k) {
  x.mapOrNull((v) => v);
  k.run();
  x.tag;
}
";
    let report = unit_report(source);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("visit"), "{rows:?}");
}
