//! The #2448 pins: the constant-pattern never-match anchor spans the FULL
//! constant node.
//!
//! Upstream anchors `constantPatternNeverMatchesValueType` at the
//! `ConstantPattern` node itself (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart,
//! `visitConstantPattern`'s `.at(node)`) — for a qualified constant that
//! is `A.a` whole (probe2443 ph: `5|10` length 3), not the inner
//! identifier the literal armings carried, and for an enum constant the
//! full pattern node, not the member tail (#2445 p7's audit finding). The
//! fix closes the constant-pattern capture before the check and anchors
//! at the captured root's span — the parsed expression itself; single-
//! token literals span their token, so the literal forms keep their
//! anchors (the #2444/#1351 pin families hold).

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

fn at(source: &str, needle: &str) -> u32 {
    u32::try_from(
        source
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {source:?}")),
    )
    .expect("offset fits u32")
}

/// Exactly one row of `name`, spanning `[offset, offset + length)` — the
/// full-node anchor both coordinates pin (the inner identifier would
/// offset-shift and the member tail would length-shrink).
fn one_span(report: &FileReport, name: &str, offset: u32, length: u32, source: &str) {
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: {rows:?}");
    assert_eq!(rows[0].range.offset, offset, "in {source:?}");
    assert_eq!(rows[0].range.length, length, "in {source:?}");
}

/// The qualified-constant face: `case A.a:` (a foreign enum's constant
/// over an enum scrutinee) reports `constant_pattern_never_matches_value_type`
/// spanning the whole `A.a` qualified node (oracle probe ph: `5|10` length
/// 3), not the inner `a` identifier the parse-time arming used to carry.
#[test]
fn qualified_constant_anchors_full_qualified_node() {
    let source = "\
enum A { a, b }
enum B { x }
void f(B e) {
  switch (e) {
    case A.a:
      print('a');
    case B.x:
      print('b');
  }
}
";
    let report = unit_report(source);
    one_span(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "A.a"),
        3,
        source,
    );
}

/// The enum-constant face: an enum constant over an unrelated-class
/// scrutinee anchors the full pattern node `E.a` (oracle #2445 p7:
/// `6|10`), not the `.a` member tail the resolution row carried.
#[test]
fn enum_constant_anchors_full_pattern_node() {
    let source = "\
enum E { a, b }
class Unrelated {}
void f(Unrelated u) {
  switch (u) {
    case E.a:
      print('a');
  }
}
";
    let report = unit_report(source);
    one_span(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "E.a"),
        3,
        source,
    );
}

/// The literal forms keep their token anchors: the captured root of a
/// single-token constant spans exactly that token, so `case true:` still
/// reports at the `true` keyword (the #2444 pin family's standing shape).
#[test]
fn literal_constant_keeps_token_anchor() {
    let source = "\
void f(int x) {
  switch (x) {
    case true:
      return 1;
    case 1:
      return 2;
  }
  return 0;
}
";
    let report = unit_report(source);
    one_span(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "true"),
        4,
        source,
    );
}
