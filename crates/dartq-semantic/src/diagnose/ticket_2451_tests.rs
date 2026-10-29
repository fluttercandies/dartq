//! The #2451 pins (census F3+F7): a closure parameter whose type flows
//! through a generic iterable chain binds from the chain's element face —
//! `m.values.map((n) => …)` reads `Map`'s second written type argument, an
//! in-package getter's `List<E>` face instantiates with the receiver's
//! written arguments, and the bound `Node?` element fires the same nullable
//! gate the direct `List<Node?>` receiver already does.
//!
//! The dropped link was the receiver classification below a qualified
//! callee: a member-access row (`m.values`) carries no receiver face of its
//! own, so the bind saw `Unknown` and left the parameter `dynamic` — every
//! member read on it silent. The upstream static type of `e.member` is the
//! getter's declared return instantiated with the receiver's type arguments
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! property_element_resolver.dart `_executeGetter`); the SDK's unindexed
//! `Map.values`/`Map.keys` ride the frozen producer table.
//!
//! Oracle provenance: every pinned shape's verdict is `dart analyze`
//! (Dart 3.13.1, fvm) on `/tmp/probe2451/pin` — the four nullable-element
//! chains and the sort/forEach controls report `unchecked_use_of_nullable_value`,
//! the non-null/dynamic-element guards stay silent, and the nullable
//! missing-member read collapses to the same unchecked row (never
//! `undefined_getter`).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Single-file rig (`main.dart` with the implicit `dart:core` seed).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_main,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    builder.seed_implicit_core(
        lib_main,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "Iterable", "void",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file_main,
        lib_main,
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

const NODE: &str = "class Node {\n\
                    \x20 int offset = 0;\n\
                    \x20 String? value;\n\
                    }\n";

/// The census F3 head shape (`incompatibleRules.values.map((node) =>
/// node.value as String)`): the closure parameter reads `Map`'s second
/// written argument — `Node?` — through the frozen `Map.values` producer,
/// and the member read fires the nullable gate exactly like the oracle's.
#[test]
fn values_chain_binds_nullable_element() {
    let source = format!(
        "{NODE}\
         void f(Map<String, Node?> m) {{\n\
         \x20 m.values.map((n) => n.value);\n\
         }}\n"
    );
    let names = code_names(&report(&source));
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "the values-chain parameter binds Node? and fires: {names:?}"
    );
}

/// The producer table's symmetric arm: `Map.keys` reads the *first*
/// written argument, so a nullable key type fires the same way.
#[test]
fn keys_chain_binds_nullable_key() {
    let source = format!(
        "{NODE}\
         void f(Map<Node?, String> m) {{\n\
         \x20 m.keys.map((k) => k.offset);\n\
         }}\n"
    );
    let names = code_names(&report(&source));
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "the keys-chain parameter binds Node? and fires: {names:?}"
    );
}

/// The in-package getter link (`libraryName.tokens.map((e) => e.lexeme)`):
/// an indexed container's declared `List<Node?>` return is the access
/// result the chained call binds from.
#[test]
fn in_package_getter_chain_binds_element() {
    let source = format!(
        "{NODE}\
         class Holder {{\n\
         \x20 List<Node?> get items => const [];\n\
         }}\n\
         void f(Holder h) {{\n\
         \x20 h.items.map((n) => n.value);\n\
         }}\n"
    );
    let names = code_names(&report(&source));
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "the getter-chain parameter binds Node?: {names:?}"
    );
}

/// The generic holder: the declared `List<T>` return instantiates with the
/// receiver's written arguments (`GHolder<Node?>` → `List<Node?>`) —
/// `greatestClosure` then answers the nullable gate, like the oracle.
#[test]
fn generic_getter_chain_instantiates_element() {
    let source = format!(
        "{NODE}\
         class GHolder<T> {{\n\
         \x20 List<T> get items => const [];\n\
         }}\n\
         void f(GHolder<Node?> h) {{\n\
         \x20 h.items.map((n) => n.value);\n\
         }}\n"
    );
    let names = code_names(&report(&source));
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "the instantiated getter-chain parameter binds Node?: {names:?}"
    );
}

/// The direct-receiver controls the census already agreed on
/// (`items.sort((a, b) => …)`, `items.forEach((e) => …)`): the chain
/// classification joins *below* the argument receiver, so the settled
/// faces keep every row.
#[test]
fn direct_receiver_controls_keep_firing() {
    let source = format!(
        "{NODE}\
         void f(List<Node?> items) {{\n\
         \x20 items.sort((a, b) => a.offset - b.offset);\n\
         \x20 items.forEach((e) => e.offset++);\n\
         }}\n"
    );
    let names = code_names(&report(&source));
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "unchecked_use_of_nullable_value")
            .count(),
        3,
        "both sort parameters and the forEach parameter fire: {names:?}"
    );
}

/// The keep-silent guard: a non-null element (`Map<String, Node>`) binds no
/// nullable recovery — the member read on the parameter stays silent, like
/// the oracle's (`n.value` exists on `Node`).
#[test]
fn non_null_element_chain_stays_silent() {
    let source = format!(
        "{NODE}\
         void f(Map<String, Node> m) {{\n\
         \x20 m.values.map((n) => n.value);\n\
         }}\n"
    );
    let names = code_names(&report(&source));
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value"),
        "a non-null element never reads nullable: {names:?}"
    );
}

/// The second keep-silent guard: a `dynamic` element never infers — the
/// member read on the parameter stays silent, like the oracle's.
#[test]
fn dynamic_element_chain_stays_silent() {
    let source = "void f(Map<String, dynamic> m) {\n\
                  \x20 m.values.map((n) => n.anything);\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value") && !names.contains(&"undefined_getter"),
        "a dynamic element's member read stays silent: {names:?}"
    );
}

/// The F7 variant's oracle shape: a missing member on a *nullable* chain
/// parameter collapses to the unchecked row (`n.valu` on `Node?` — the
/// nullability gate fires before any member lookup), never
/// `undefined_getter`.
#[test]
fn nullable_missing_member_collapses_to_unchecked() {
    let source = format!(
        "{NODE}\
         void f(Map<String, Node?> m) {{\n\
         \x20 m.values.map((n) => n.valu);\n\
         }}\n"
    );
    let names = code_names(&report(&source));
    assert!(
        names.contains(&"unchecked_use_of_nullable_value") && !names.contains(&"undefined_getter"),
        "the nullable gate fires and no undefined_getter joins: {names:?}"
    );
}
