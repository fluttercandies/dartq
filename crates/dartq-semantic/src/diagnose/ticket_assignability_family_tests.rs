//! The assignability-family W3 differential pins: the faces the real-corpus
//! burn (binary 298957be, 17 packages) proved divergent, verified end-to-end
//! through the single-file rig.
//!
//! - The `FutureOr<T>` union rule: `S <: FutureOr<T>` iff `S <: Future<T>`
//!   or `S <: T` — `Null` into `FutureOr<C?>` rides the value leg (the
//!   oracle's `Null <: FutureOr<ResponsePayload?>` stays clean), `Null`
//!   into `FutureOr<C>` reports.
//! - The async matrix's Right Bottom leg: `Future<void> f() async => throw`
//!   — a `Never` expression assigns into every declared shape before the
//!   flatten projections ask their own questions.
//! - The strict function-shape slots: the contravariant parameter leg asks
//!   `dynamic <: Node` (false) — the expression-position downcast leniency
//!   (`dynamic → C`) never enters a type's shape.
//! - The covariant-interface element rule: `Box<dynamic>` does not fit
//!   `Box<int>` — same strictness, generic arguments included.
//! - The record poison: an `InvalidType` field silences the whole-record
//!   question, the other fields' nullable-vs-non-nullable mismatch
//!   included.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the `tests.rs` `report` rig mirrored (the sibling rig stays off-
/// limits while its lane is in flight).
fn report(source: &str) -> FileReport {
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
    let fold = fold_unit(
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
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

// —— the FutureOr<T> union rule ——————————————————————————————————————

/// The union faces key on the head spelling (`FutureOr`), declared
/// locally: the rig carries no dart:async, and the subtype question is
/// spelling-keyed the same way the corpus faces are.
const FUTURE_OR_STUBS: &str = "class FS {}
class V {}
class Future<T> {}\n\
                                 class FutureOr<T> {}\n\
                                 class C {}\n";

/// `Null` into `FutureOr<C?>` rides the value leg (`Null <: C?`) and
/// stays silent — the corpus `FutureOr<ResponsePayload?>` return face.
#[test]
fn null_into_future_or_of_nullable_value_is_clean() {
    let source = format!("{FUTURE_OR_STUBS}FutureOr<C?> f() {{\n  return null;\n}}\n");
    let names = code_names(&report(&source));
    assert!(
        !names.iter().any(|c| *c == "return_of_invalid_type"),
        "{names:?}"
    );
}

/// `Null` into `FutureOr<C>` fails both legs (the value leg's `C` is
/// non-nullable, the future leg's `Future<C>` likewise) and reports.
#[test]
fn null_into_future_or_of_non_nullable_value_reports() {
    let source = format!("{FUTURE_OR_STUBS}FutureOr<C> f() {{\n  return null;\n}}\n");
    let names = code_names(&report(&source));
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "return_of_invalid_type")
            .count(),
        1,
        "{names:?}"
    );
}

// —— the async matrix's Right Bottom leg —————————————————————————————

/// `async => throw` — the expression's `Never` assigns into the declared
/// `Future<void>` (the corpus `Future<void> _defaultOnUpdate(...) async
/// => throw ...` face); the flatten ok-sets never name `Never`, so the
/// bottom leg must answer first.
#[test]
fn async_arrow_throw_of_never_is_clean() {
    let source = "class Future<T> {}\n\
                      Future<void> f() async => throw 'x';\n";
    let names = code_names(&report(&source));
    assert!(
        !names.iter().any(|c| *c == "return_of_invalid_type"),
        "{names:?}"
    );
}

// —— the strict function-shape slots ——————————————————————————————————

/// A tear-off whose parameter type does not admit the target's
/// `dynamic` parameter reports: the contravariant leg asks
/// `dynamic <: Node` — false (the corpus `.map(XPathNode.new)` against
/// an `XPathItem Function(dynamic)` face). The downcast leniency that
/// lets a `dynamic` expression assign into `C` never applies inside a
/// function type's shape.
#[test]
fn tear_off_contravariant_dynamic_parameter_reports() {
    let source = "class Item {}\n\
                      class Node extends Item {}\n\
                      Item nodeMake(Node n) => n;\n\
                      void take(Item Function(dynamic x) f) {}\n\
                      void use() {\n  take(nodeMake);\n}\n";
    let names = code_names(&report(&source));
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "argument_type_not_assignable")
            .count(),
        1,
        "{names:?}"
    );
}

/// The matching clean face: a parameter-compatible tear-off stays
/// silent (`Item <: Item` on the return, `Node <: Item`—admitting
/// `dynamic` callers is the source's own problem — here the target's
/// `Node` parameter is a subtype of the source's `Item`).
#[test]
fn tear_off_compatible_slots_stay_silent() {
    let source = "class Item {}\n\
                      class Node extends Item {}\n\
                      Node nodeMake(Item i) => i as Node;\n\
                      void take(Node Function(Node n) f) {}\n\
                      void use() {\n  take(nodeMake);\n}\n";
    let names = code_names(&report(&source));
    assert!(
        !names.iter().any(|c| *c == "argument_type_not_assignable"),
        "{names:?}"
    );
}

// —— the covariant-interface element rule —————————————————————————————

/// `Box<dynamic>` does not fit `Box<int>`: the generic-argument
/// covariance asks `dynamic <: int` — false (the corpus
/// `StateController<dynamic>` against `StateController<ValueT>`).
/// The assignable face's dynamic downcast leniency must not answer
/// inside the argument shape.
#[test]
fn dynamic_argument_covariance_reports() {
    let source = "class Box<T> {}\n\
                      void takeBox(Box<int> b) {}\n\
                      void use(Box<dynamic> d) {\n  takeBox(d);\n}\n";
    let names = code_names(&report(&source));
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "argument_type_not_assignable")
            .count(),
        1,
        "{names:?}"
    );
}

/// The clean covariance faces: identical arguments stay silent, and an
/// unresolvable element relation stays the under-report (the
/// `int`-into-declared-head pair the primitive table cannot decide).
#[test]
fn compatible_covariant_arguments_stay_silent() {
    let identical = "class Box<T> {}\n\
                         void takeBox(Box<int> b) {}\n\
                         void use(Box<int> b) {\n  takeBox(b);\n}\n";
    let names = code_names(&report(identical));
    assert!(
        !names.iter().any(|c| *c == "argument_type_not_assignable"),
        "{names:?}"
    );
    let undecidable = "class Box<T> {}\n\
                           class Num {}\n\
                           void takeNum(Box<Num> b) {}\n\
                           void use(Box<int> b) {\n  takeNum(b);\n}\n";
    let names = code_names(&report(undecidable));
    assert!(
        !names.iter().any(|c| *c == "argument_type_not_assignable"),
        "{names:?}"
    );
}

// —— the record poison ————————————————————————————————————————————————

/// A record source with an `InvalidType` field stays silent on the
/// whole-record question — the other field's nullable-vs-non-nullable
/// mismatch included (the corpus `(FeatureSet?, InvalidType)` against
/// `(FeatureSet, Version)` return face). The unresolved `Zbad` spells
/// the oracle's `InvalidType` slot in the source shape.
#[test]
fn invalid_type_field_poisons_the_record_question() {
    let source = "class FS {}\n\
                      class V {}\n\
                      (FS, V) mk() {\n  FS? fs;\n  return (fs, zbad);\n}\n";
    let names = code_names(&report(&source));
    assert!(
        !names.iter().any(|c| *c == "return_of_invalid_type"),
        "{names:?}"
    );
}

/// A fully-typed record pair keeps its field checks: the nullable first
/// field against a non-nullable host reports (the poison arm must not
/// swallow decidable questions).
#[test]
fn fully_typed_record_nullable_field_reports() {
    let source = "class FS {}\n\
                      class V {}\n\
                      (FS, V) mk() {\n  (FS?, V) r = (FS(), V());\n  return r;\n}\n";
    let names = code_names(&report(&source));
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "return_of_invalid_type")
            .count(),
        1,
        "{names:?}"
    );
}
