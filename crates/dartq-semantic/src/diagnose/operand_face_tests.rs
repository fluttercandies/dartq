//! The operand-face pins (`diagnose/operand_face.rs`): the numeric
//! refinement reads an operand the engine cannot type as a GAP that
//! suppresses the fold, not as the oracle's `InvalidType`, while a
//! genuine resolution failure keeps deciding the `double` branch.
//!
//! Oracle provenance (`dart analyze` 3.13.1): an untyped `static const`
//! field infers `int` from its initializer, so `v + _n` is `int` and the
//! `int` sinks stay silent; `int - <dynamic>` is `num` (subtype.dart's
//! Left Top rule: `Object? <: double?` and `Object? <: int?` both fail),
//! so the `int` AND `double` sinks report with 'num' in the message.

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
/// `package:t`, no imports (`ticket_2453_tests` carries the same shape).
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

fn assignability_codes(report: &FileReport) -> Vec<&str> {
    codes(&report)
        .into_iter()
        .filter(|c| matches!(*c, "argument_type_not_assignable" | "invalid_assignment"))
        .collect()
}

/// The archive corpus shape (`bzip2_encoder.dart`: `static const
/// _bzNOvershoot = (_bzNRadix + _bzNQSort + _bzNShell + 2);` declared at
/// the class tail, read in the bodies above it): the untyped field is a
/// gap, not an InvalidType, so `n + _n` never folds to `double` and the
/// `int` sinks stay silent — bare, qualified, compound, declared before
/// AND after the reads alike.
#[test]
fn untyped_static_const_field_operand_declines_the_refinement() {
    let source = "\
void g({required int length}) {}

class E {
  static const _early = 3;

  void f(int n) {
    g(length: n + _late);
    g(length: n + E._late);
    g(length: n * _early);
    int a = n - _late;
    int b = _late + n;
    int total = 0;
    total += _late;
    total -= _early;
    a = b + total;
  }

  static const _late = (_early + 2);
}
";
    let report = unit_report(source);
    assert_eq!(
        assignability_codes(&report),
        Vec::<&str>::new(),
        "{:?}",
        codes(&report)
    );
}

/// A declared member the model carries untyped (`var count = 0;` — the
/// oracle infers `int`) reads as a gap through a typed receiver too.
#[test]
fn untyped_member_read_on_a_resolved_receiver_declines() {
    let source = "\
void g({required int length}) {}

class Node {
  var count = 0;
}

void f(Node node, int n) {
  g(length: n + node.count);
  int a = node.count - n;
  a += node.count;
}
";
    let report = unit_report(source);
    assert_eq!(
        assignability_codes(&report),
        Vec::<&str>::new(),
        "{:?}",
        codes(&report)
    );
}

/// The resolution failures keep the oracle's InvalidType face: an
/// undefined bare name and a member read through an unresolvable written
/// head both decide the `double` branch, and the `int` sinks report.
#[test]
fn resolution_failures_still_type_double() {
    let source = "\
void g({required int length}) {}

class Node {
  int end = 0;
}

void f(Node node, Token operator, int n) {
  g(length: n + missing);
  g(length: node.end - operator.offset);
}
";
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        [
            "The argument type 'double' can't be assigned to the parameter type 'int'. ",
            "The argument type 'double' can't be assigned to the parameter type 'int'. ",
        ],
        "{:?}",
        codes(&report)
    );
}

/// `int - <dynamic>` is `num` (probed 3.13.1 — Left Top: `dynamic <:
/// double?` and `dynamic <: int?` both fail, neither branch fires): the
/// `int` sink reports 'num', and so does the `double` sink the old
/// `double` reading left silent.
#[test]
fn int_op_dynamic_types_num_at_both_sinks() {
    let source = "\
void g({required int length}) {}
void h({required double d}) {}

class Node {
  int end = 0;
}

void f(Node node, dynamic dy) {
  g(length: node.end - dy);
  h(d: node.end - dy);
  double b = node.end * dy;
}
";
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        [
            "The argument type 'num' can't be assigned to the parameter type 'int'. ",
            "The argument type 'num' can't be assigned to the parameter type 'double'. ",
        ],
        "{:?}",
        codes(&report)
    );
    assert_eq!(
        messages(&report, "invalid_assignment"),
        ["A value of type 'num' can't be assigned to a variable of type 'double'."],
        "{:?}",
        codes(&report)
    );
}
