//! The Object-face send typing over container-less receivers (#2521): an
//! assignment whose right side is an instance-member send on a receiver the
//! classifier cannot name a container for (a function value, a `dynamic`)
//! writes the member's declared return — `Object`'s own face — so the
//! target promotes through the write exactly as the official resolver's
//! chain does, and the downstream `argument_type_not_assignable` verdict
//! the parse-time face staged stays cleaned.
//!
//! Oracle provenance (`dart analyze` 3.13.1): `s = callback.toString()`
//! and `s ??= kReleaseMode ? 'compute' : callback.toString()` both leave
//! `s` promoted to `String` (the write carries the send's resolved static
//! type), and `d.toString()` on a `dynamic` receiver types `String` — a
//! following `s.length` reports nothing, so the send never types
//! `dynamic`. The unpromoted control keeps the one diagnostic.

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
/// `package:t`, no imports (`operand_face_tests` carries the same shape).
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

/// The dio corpus shape (`compute_io.dart`: `debugLabel ??= kReleaseMode
/// ? 'compute' : callback.toString()` then `debugLabel` as a non-null
/// argument): the conditional's else-branch send types `String` through
/// the function value's `Object` face, the if-null write promotes, and
/// the staged argument verdict stays cleaned.
#[test]
fn if_null_conditional_send_rhs_promotes() {
    let source = r#"
const bool kReleaseMode = true;

typedef Callback = void Function();

class Config {
  Config(String label);
}

void f(Callback callback, String? debugLabel) {
  debugLabel ??= kReleaseMode ? 'compute' : callback.toString();
  Config(debugLabel);
}
"#;
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        Vec::<String>::new(),
        "{:?}",
        codes(&report)
    );
}

/// A plain write over the same send (`s = callback.toString()`): the
/// write stages the send's return and the read at the call site carries
/// the promoted type.
#[test]
fn plain_send_rhs_promotes() {
    let source = r#"
typedef Callback = void Function();

class Config {
  Config(String label);
}

void f(Callback callback, String? s) {
  s = callback.toString();
  Config(s);
}
"#;
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        Vec::<String>::new(),
        "{:?}",
        codes(&report)
    );
}

/// The direct if-null form (`s ??= callback.toString()`): no conditional
/// around the send, the same promotion.
#[test]
fn if_null_direct_send_rhs_promotes() {
    let source = r#"
typedef Callback = void Function();

class Config {
  Config(String label);
}

void f(Callback callback, String? s) {
  s ??= callback.toString();
  Config(s);
}
"#;
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        Vec::<String>::new(),
        "{:?}",
        codes(&report)
    );
}

/// A `dynamic` receiver's `Object` member resolves statically (`d
/// .toString()` types `String`, probed 3.13.1: a following `s.length`
/// reports nothing, so the send never types `dynamic`), and the write
/// promotes through it.
#[test]
fn dynamic_receiver_object_send_promotes() {
    let source = r#"
class Config {
  Config(String label);
}

void f(dynamic d, String? s) {
  s = d.toString();
  Config(s);
}
"#;
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        Vec::<String>::new(),
        "{:?}",
        codes(&report)
    );
}

/// The `hashCode` twin of the same face: `int` is `Object`'s declared
/// return, so the write promotes the nullable `int` target.
#[test]
fn object_hash_code_send_promotes() {
    let source = r#"
typedef Callback = void Function();

class Config {
  Config(int id);
}

void f(Callback callback, int? id) {
  id ??= callback.hashCode;
  Config(id);
}
"#;
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        Vec::<String>::new(),
        "{:?}",
        codes(&report)
    );
}

/// The intl corpus chain (`number_format.dart:394-406`): a `var`-inferred
/// local through a core-map index and a trailing `!` carries the map's
/// value type (`var symbols = numberFormatSymbols[locale]!` infers
/// `NumberSymbols`), so the field read `symbols.DEF_CURRENCY_CODE` types
/// `String`, the first `??=` promotes `name`, and the second `??=` over
/// the promoted read promotes `currencySymbol` — the non-null argument
/// at the factory call stays clean.
#[test]
fn map_index_bang_var_chain_promotes() {
    let source = r#"
class NumberSymbols {
  final String DEF_CURRENCY_CODE;
  const NumberSymbols({required this.DEF_CURRENCY_CODE});
}

const Map<String, NumberSymbols> numberFormatSymbols = {
  'x': NumberSymbols(DEF_CURRENCY_CODE: 'USD'),
};

class Config {
  Config(String label);
}

void f(String? locale, {String? name, String? currencySymbol}) {
  var symbols = numberFormatSymbols[locale]!;
  name ??= symbols.DEF_CURRENCY_CODE;
  currencySymbol ??= name;
  Config(currencySymbol);
}
"#;
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        Vec::<String>::new(),
        "{:?}",
        codes(&report)
    );
}

/// The unpromoted control: without the write, the nullable argument keeps
/// its one diagnostic — the pins above clean a real staged verdict, not a
/// face that never fires.
#[test]
fn unpromoted_control_still_reports() {
    let source = r#"
typedef Callback = void Function();

class Config {
  Config(String label);
}

void f(Callback callback, String? debugLabel) {
  Config(debugLabel);
}
"#;
    let report = unit_report(source);
    assert_eq!(
        messages(&report, "argument_type_not_assignable"),
        ["The argument type 'String?' can't be assigned to the parameter type 'String'. "],
        "{:?}",
        codes(&report)
    );
}
