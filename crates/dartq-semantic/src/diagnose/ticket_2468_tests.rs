//! The #2468 pins: the constant-pattern machinery arms for CLASS static
//! consts, not only enum constants.
//!
//! `case A.a:` over a class `A { static const a = 1; }` never armed —
//! [`classify_constant_tail`] mapped only enum containers, so the tail
//! fell through and the never-match face stayed silent while the oracle
//! reports (probe2468 p1: `warning 7:10 constant_pattern_never_matches_
//! value_type`, `'String'` vs `'int'`). Upstream evaluates the reference
//! and reads `value.type` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart,
//! `visitConstantPattern`), gating on `hasPrimitiveEquality` — the name
//! level answers from the captured constant value for the shapes whose
//! primitive equality is unconditional (the primitives and enum values),
//! and keeps the under-report for user-class values (an `==`/`hashCode`
//! override stays silent upstream — probe2468 p6 shows the plain-class
//! row the `Created` face cannot soundly arm). The `const`-prefixed
//! spelling anchors at the keyword (probe2468 p3: `warning 7:10` with the
//! pattern written `case const A.a:` — the ConstantPattern node spans
//! `const` through the expression), not at the capture root's `A`.

use super::*;
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

/// Exactly one row of `name`, warning-severity, spanning
/// `[offset, offset + length)` — the oracle's STATIC_WARNING tier and
/// node anchor both pin (the inner identifier would offset-shift, the
/// member tail would length-shrink, and the un-prefixed capture root
/// would miss the `const` keyword).
fn one_row(report: &FileReport, name: &str, offset: u32, length: u32, source: &str) {
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: {rows:?}");
    assert_eq!(rows[0].severity, Severity::Warning, "in {source:?}");
    assert_eq!(rows[0].range.offset, offset, "in {source:?}");
    assert_eq!(rows[0].range.length, length, "in {source:?}");
}

/// The class-static-const face: `case A.a:` arms the constant machinery —
/// the folded `int` value types the constant and the never-matching
/// `String` scrutinee reports `constant_pattern_never_matches_value_type`
/// spanning the whole `A.a` qualified node (oracle probe p1: `warning
/// 7:10`, `'String'` can never equal `'int'`), the row the enum-only tail
/// classification used to swallow. The rig declares the primitive class
/// the unit world carries no dart:core for (the tests.rs rig pattern) —
/// the oracle's real run has it built in, and with it the row set here is
/// exactly the oracle's single row.
#[test]
fn class_static_const_never_match_reports_full_qualified_node() {
    let source = "\
class String {}

class A {
  static const a = 1;
}

void f(String s) {
  switch (s) {
    case A.a:
      break;
  }
}
";
    let report = unit_report(source);
    one_row(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "A.a"),
        3,
        source,
    );
}

/// The matching scrutinee stays silent: `int x` against the folded `int`
/// constant is provably possible equality — the oracle reports nothing
/// (probe p2: `No issues found`) and neither does the engine (the rig
/// declares the `int` class the unit world lacks).
#[test]
fn class_static_const_matching_scrutinee_stays_silent() {
    let source = "\
class int {}

class A {
  static const a = 1;
}

void f(int x) {
  switch (x) {
    case A.a:
      break;
  }
}
";
    let report = unit_report(source);
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

/// The enum-constant form keeps its row (the #2448 regression guard): an
/// enum constant over an unrelated-class scrutinee still reports at the
/// full pattern node `E.a` — the class-container extension must not have
/// disturbed the enum tail it mirrors.
#[test]
fn enum_constant_form_unchanged() {
    let source = "\
enum E { a, b }

class Unrelated {}

void f(Unrelated u) {
  switch (u) {
    case E.a:
      break;
  }
}
";
    let report = unit_report(source);
    one_row(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "E.a"),
        3,
        source,
    );
}

/// The `const`-prefixed spelling anchors at the keyword: the
/// ConstantPattern node spans `const A.a` (oracle probe p3: `warning
/// 7:10` for a pattern written at column 10 — the keyword), not the
/// capture root's `A.a` the expression-only span used to carry. (The
/// oracle's companion `invalid_constant_const_prefix` error row is the
/// eval face's unimplemented code — the documented gap this ticket does
/// not open.)
#[test]
fn const_prefixed_class_const_anchors_at_keyword() {
    let source = "\
class String {}

class A {
  static const a = 1;
}

void f(String s) {
  switch (s) {
    case const A.a:
      break;
  }
}
";
    let report = unit_report(source);
    one_row(
        &report,
        "constant_pattern_never_matches_value_type",
        at(source, "const A.a"),
        9,
        source,
    );
}
