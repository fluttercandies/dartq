//! The #2453 pins: the num-fallback binary-operator typing over an
//! invalid/dynamic operand (census F5).
//!
//! The oracle never degrades a resolved-left arithmetic fold when the
//! right operand's type is `InvalidType`/`dynamic`:
//! `_resolveUserDefinableType` answers the resolved operator's declared
//! return and `_refineNumericInvocationTypeNullSafe` (upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/element/type_system.dart:2059-2121)
//! refines it — `InvalidType` is a subtype of everything (subtype.dart's
//! `InvalidTypeImpl` arm), so the `S <: double?` branch fires and
//! `int - <invalid>` types `double` (then the ordinary assignability
//! faces report against `int` contexts); `dynamic` is Left Top and fires
//! neither branch, so `int - <dynamic>` types `num`. The operators
//! outside the refinement set (`~/`, `/`, `&`, relational) keep the
//! declared return, and a receiver outside `num` (`String`) keeps its own.
//!
//! Oracle provenance: `dart analyze` 3.13.1 probes (`int {+,-,*,%}
//! invalid` → `double`, `int / invalid` → `double`, `int ~/ invalid` →
//! `int`, `num - invalid` → `double`, `String + invalid` → `String`,
//! `int - dynamic` → `num`, invalid-left and relational silent,
//! `int total; total += <invalid>` → `invalid_assignment` 'double',
//! the undefined-callee `mystery()` face is InvalidType too, answers
//! `double` and reports `return_of_invalid_type`).

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

/// The corpus shape (`dead_code_verifier.dart:500`,
/// `g(length: node.end - operator.offset)` with `Token` undefined): the
/// fold answers `double` and the named-argument face reports
/// `argument_type_not_assignable` 'double'-against-'int' (the row dartq
/// used to stay silent on).
#[test]
fn int_minus_invalid_types_double_and_reports_argument() {
    let source = "\
class Node {
  int end = 0;
}

void g({required int length}) {}

void f(Node node, Token operator) {
  g(length: node.end - operator.offset);
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        1,
        "{:?}",
        codes(&report)
    );
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        ["The argument type 'double' can't be assigned to the parameter type 'int'. "],
        "{:?}",
        messages(&report, "argument_type_not_assignable")
    );
}

/// The refinement set is `{+,-,*,%}`: every one of them answers `double`
/// over an invalid right operand (p_matrix lines 18-20).
#[test]
fn int_plus_times_percent_invalid_all_double() {
    let source = "\
class Node {
  int end = 0;
}

void g({required int length}) {}

void f(Node node, Token operator) {
  g(length: node.end + operator.offset);
  g(length: node.end * operator.offset);
  g(length: node.end % operator.offset);
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

/// `~/` sits outside the refinement set: the declared `int` return
/// stands, so the `int`-formal call stays silent over the invalid
/// operand — no over-report (p_matrix line 22 checks the declared face
/// through the oracle's `double`-formal probe; the call face for a
/// non-refined fold row is the under-report direction here).
#[test]
fn int_tilde_slash_invalid_keeps_declared_int() {
    let source = "\
class Node {
  int end = 0;
}

void g({required int length}) {}

void f(Node node, Token operator) {
  g(length: node.end ~/ operator.offset);
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

/// `/` declares `double` itself; the invalid operand changes nothing
/// (p_matrix line 21).
#[test]
fn int_slash_invalid_keeps_declared_double() {
    let source = "\
class Node {
  int end = 0;
}

void g({required int length}) {}

void f(Node node, Token operator) {
  g(length: node.end / operator.offset);
}
";
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        ["The argument type 'double' can't be assigned to the parameter type 'int'. "],
        "{:?}",
        messages(&report, "argument_type_not_assignable")
    );
}

/// A receiver outside `num` never refines: `String + <invalid>` answers
/// the declared `String` (p_str line 12).
#[test]
fn string_plus_invalid_keeps_string() {
    let source = "\
class Node {
  String s = '';
}

void g({required int length}) {}

void f(Node node, Token operator) {
  g(length: node.s + operator.offset);
}
";
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        ["The argument type 'String' can't be assigned to the parameter type 'int'. "],
        "{:?}",
        messages(&report, "argument_type_not_assignable")
    );
}

/// `dynamic` is Left Top, not a subtype of everything: `dynamic <:
/// double?` and `dynamic <: int?` both fail (subtype.dart's Left Top rule
/// asks `Object? <: T1`), so neither refinement branch fires and `int -
/// <dynamic>` types `num` — the `int` sink reports with 'num' (probed
/// 3.13.1; the earlier 'double' reading was wrong and left the `double`
/// sinks silent, see `operand_face_tests`).
#[test]
fn int_minus_dynamic_types_num() {
    let source = "\
class Node {
  int end = 0;
}

void g({required int length}) {}

void f(Node node, dynamic dy) {
  g(length: node.end - dy);
}
";
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        ["The argument type 'num' can't be assigned to the parameter type 'int'. "],
        "{:?}",
        messages(&report, "argument_type_not_assignable")
    );
}

/// A `num` receiver refines through the same `S <: double?` branch:
/// `num - <invalid>` answers `double` (p_matrix line 30 — plain `num`
/// arithmetic over a known-`int` right stays `num`, the invalid side is
/// what decides `double` here).
#[test]
fn num_minus_invalid_types_double() {
    let source = "\
class Node {
  num n = 0;
}

void g({required int length}) {}

void f(Node node, Token operator) {
  g(length: node.n - operator.offset);
}
";
    let report = unit_report(source);
    assert!(
        messages(&report, "argument_type_not_assignable")
            .iter()
            .all(|m| m.contains("'double'")),
        "{:?}",
        messages(&report, "argument_type_not_assignable")
    );
}

/// The under-report face is preserved: an invalid LEFT leaves the
/// operator unresolved (upstream answers `InvalidType`) and every
/// downstream check stays silent (p_matrix lines 37-40).
#[test]
fn invalid_left_stays_silent() {
    let source = "\
class Node {
  int end = 0;
}

void g({required double length}) {}

void f(Node node, Token operator) {
  g(length: operator.offset - node.end);
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

/// The compound face (`assignment_expression_resolver.dart:308-313`):
/// `int total; total += <invalid>` types the read `total + <invalid>` as
/// `double` and reports `invalid_assignment` (p_matrix line 43 — the
/// manifest_ast.dart:158 census row); the `num` target accepts the
/// refined `double` silently (p_matrix line 45).
#[test]
fn compound_assignment_reports_invalid_assignment() {
    let source = "\
void f(Token operator) {
  int total = 0;
  total += operator.offset;
  num totalN = 0;
  totalN -= operator.offset;
}
";
    let report = unit_report(source);
    assert_eq!(
        count_code(&report, "invalid_assignment"),
        1,
        "{:?}",
        codes(&report)
    );
    assert_eq!(
        messages(&report, "invalid_assignment"),
        ["A value of type 'double' can't be assigned to a variable of type 'int'."],
        "{:?}",
        messages(&report, "invalid_assignment")
    );
}

/// Equality stays `bool` over an invalid operand (p_matrix line 27) —
/// the refinement never touches the `==` composite.
#[test]
fn equality_over_invalid_stays_bool() {
    let source = "\
class Node {
  int end = 0;
}

void g({required bool flag}) {}

void f(Node node, Token operator) {
  g(flag: node.end == operator.offset);
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

/// The shadowed synthetic world (the #2172 rig): a unit-declared
/// `class int` owning its `operator +` has no `num` face — the fold
/// answers the declared return and the return check stays silent
/// (the real oracle resolves the user operator and skips the
/// refinement; `undefined_method` is the only row).
#[test]
fn shadowed_int_world_answers_declared() {
    let source = "\
class int {
  int operator +(int other) => this;
}

class C {
  int t = 0;
  int f() {
    var fs = mystery() - 1;
    return t + fs;
  }
}
";
    let report = unit_report(source);
    assert_eq!(codes(&report), ["undefined_method"], "{:?}", codes(&report));
}
