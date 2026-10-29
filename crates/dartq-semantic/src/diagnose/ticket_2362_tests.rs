//! The #2362 differential pins: the generic-supertype instantiation's
//! whole-`Ty::Named` unwrap. The clause scanner stores every super face as
//! `(head, vec![whole_named_ty])`, so `instantiate_inherited` must bind the
//! declaring hop's parameters to the stored `Ty::Named`'s own `args` —
//! never to the one-element wrapping. Binding the wrapping (`XmlNodeList<E>
//! extends DelegatingList<E>` bound `E ↦ DelegatingList<E>`) rendered every
//! inherited signature through the super face itself and false-fired
//! `invalid_override` on members the oracle accepts (the #2352
//! `node_list` probe). The concrete-instantiation faces are the
//! discriminating pins — red before the fix, green after — while the
//! bounded generic-parameter faces pin the no-false-positive contract (a
//! bare parameter face stays undecidable in the assignability fallback, so
//! it cannot discriminate by itself). Each fixture was probed against
//! oracle `dart analyze` (fvm default).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module. `dart:core` is seeded
/// implicitly with the unindexed face, like the engine driver does.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    builder.seed_implicit_core(
        lib,
        core_file,
        &["Object", "String", "int", "bool", "double", "num"],
    );
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

/// Diagnoses `main_source` as one file of a two-file library — `other`
/// declares the superclass `main` extends, foreign to `main`'s diagnosis
/// pool (the import edge `main → other` resolves the hop). `dart:core` is
/// seeded implicitly with the unindexed face, like the engine driver does —
/// an explicit import edge leaves the prelude unseeded otherwise.
fn report_two_file(main_source: &str, other_source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_other = files.intern(Path::new("other.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    const CORE: &[&str] = &["Object", "String", "int", "bool", "double", "num"];
    let scan = |source: &str, file: FileId| {
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
        scanned
    };
    let scanned_main = scan(main_source, file_main);
    let scanned_other = scan(other_source, file_other);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main_source, &scanned_main.tokens, features, &strings);
    let elements_other = collect(other_source, &scanned_other.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_other = builder.intern("package:t/other.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_other, elements_other.clone());
    for (name, decl) in elements_main.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    for (name, decl) in elements_other.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_other, name, file_other);
    }
    builder.seed_implicit_core(lib_main, core_file, CORE);
    builder.seed_implicit_core(lib_other, core_file, CORE);
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_other),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main_source,
        &scanned_main.tokens,
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

fn assert_clean(source: &str) {
    let names = code_names(&report(source));
    assert!(
        !names.contains(&"invalid_override"),
        "no invalid_override may report: {names:?}"
    );
}

/// (a) The same-file single-argument face over a generic subclass:
/// `Sub<X> extends Box<X>`'s overrides compare against `Box<T>`'s members
/// instantiated `T ↦ X` — binding the hop's parameter to the whole
/// `Box<X>` face (the one-element wrapping) rendered the inherited
/// signature `Box<X>`. The bounded parameters are what make the oracle
/// comparison well-defined (upstream verifies each fixture clean).
#[test]
fn same_file_generic_face_binds_the_real_argument() {
    assert_clean(
        "class Num {}\n\
         class Box<T> {\n\
         \x20 void add(T value) {}\n\
         \x20 T get first => throw '';\n\
         }\n\
         class Sub<X extends Num> extends Box<X> {\n\
         \x20 void add(X value) {}\n\
         \x20 X get first => throw '';\n\
         }\n",
    );
}

/// (a2) The discriminating concrete face: `Sub extends Box<int>`
/// instantiates the inherited `T` to `int` — the wrapping binding instead
/// rendered `Box<int>` and false-fired the valid overrides.
#[test]
fn same_file_concrete_face_instantiates_the_written_argument() {
    assert_clean(
        "class Box<T> {\n\
         \x20 void add(T value) {}\n\
         \x20 T get first => throw '';\n\
         }\n\
         class Sub extends Box<int> {\n\
         \x20 void add(int value) {}\n\
         \x20 int get first => throw '';\n\
         }\n",
    );
}

/// (b) The non-generic face keeps the plain comparison: a matching
/// override stays silent, a genuinely mismatched one still reports — the
/// unwrap must not suppress faces that carry no type arguments.
#[test]
fn non_generic_super_keeps_the_plain_comparison() {
    let rep = report(
        "class Box {\n\
         \x20 void add(int value) {}\n\
         }\n\
         class Good extends Box {\n\
         \x20 void add(int value) {}\n\
         }\n\
         class Bad extends Box {\n\
         \x20 void add(String value) {}\n\
         }\n",
    );
    let overrides: Vec<_> = rep
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .collect();
    assert_eq!(
        overrides.len(),
        1,
        "only Bad's mismatch may report: {:?}",
        rep.diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
}

/// (c) The nested-argument face over a generic subclass:
/// `Sub<E> extends Outer<Inner<E>>` instantiates the inherited `T? get
/// first` to `Inner<E>?` — the substitution reads the stored face's own
/// argument list, so the written override's nested spelling compares equal.
#[test]
fn nested_generic_argument_substitutes_into_the_member() {
    assert_clean(
        "class Base {}\n\
         class Inner<A> {}\n\
         class Outer<T> {\n\
         \x20 T? get first => throw '';\n\
         }\n\
         class Sub<E extends Base> extends Outer<Inner<E>> {\n\
         \x20 Inner<E>? get first => throw '';\n\
         }\n",
    );
}

/// (c2) The discriminating concrete nested face: `Sub extends
/// Outer<Inner>` instantiates the inherited `T get first` to `Inner` —
/// the wrapping binding instead rendered `Outer<Inner>` and false-fired.
/// (The nullable twin stays undecidable in the fallback and cannot
/// discriminate; the non-nullable heads answer the lattice question.)
#[test]
fn concrete_nested_face_substitutes_the_written_argument() {
    assert_clean(
        "class Inner {}\n\
         class Outer<T> {\n\
         \x20 T get first => throw '';\n\
         }\n\
         class Sub extends Outer<Inner> {\n\
         \x20 Inner get first => throw '';\n\
         }\n",
    );
}

/// (d) The foreign name-forced hop — the #2352 probe's `node_list` shape:
/// `XmlNodeList<E extends XmlNode> extends DelegatingList<E>` over an
/// imported `DelegatingList<T>` instantiates the foreign single-parameter
/// hop's `T` to the written argument `E`, so `add`/`insert`/`[]` all
/// compare equal. Binding `T ↦ DelegatingList<E>` (the wrapping)
/// false-fired the bounded-parameter faces on every inherited member.
#[test]
fn foreign_generic_super_substitutes_across_the_import_edge() {
    let other = "class DelegatingList<T> {\n\
                 \x20 void add(T value) {}\n\
                 \x20 void insert(int index, T value) {}\n\
                 \x20 T operator [](int index) => throw '';\n\
                 }\n";
    let main = "import 'other.dart';\n\
                class XmlNode {}\n\
                class XmlNodeList<E extends XmlNode> extends DelegatingList<E> {\n\
                \x20 void add(E value) {}\n\
                \x20 void insert(int index, E value) {}\n\
                \x20 E operator [](int index) => throw '';\n\
                }\n";
    let names = code_names(&report_two_file(main, other));
    assert!(
        !names.contains(&"invalid_override"),
        "XmlNodeList's members override DelegatingList<E> instantiated with E: {names:?}"
    );
}

/// (d2) The discriminating concrete foreign face: `NodeList extends
/// DelegatingList<Node>` instantiates the foreign hop's `T` to `Node` —
/// the wrapping binding instead rendered `DelegatingList<Node>` and
/// false-fired both the parameter and the return faces.
#[test]
fn foreign_concrete_face_substitutes_across_the_import_edge() {
    let other = "class DelegatingList<T> {\n\
                 \x20 void add(T value) {}\n\
                 \x20 T get first => throw '';\n\
                 }\n";
    let main = "import 'other.dart';\n\
                class Node {}\n\
                class NodeList extends DelegatingList<Node> {\n\
                \x20 void add(Node value) {}\n\
                \x20 Node get first => throw '';\n\
                }\n";
    let names = code_names(&report_two_file(main, other));
    assert!(
        !names.contains(&"invalid_override"),
        "NodeList's members override DelegatingList<Node> instantiated: {names:?}"
    );
}

/// (e) The foreign hop's substitution feeds a real comparison: with the
/// face instantiated `T ↦ int`, a `String`-parameter override still
/// reports — the unwrap must not blanket-suppress the edge.
#[test]
fn foreign_face_substitution_keeps_the_real_mismatch_reporting() {
    let other = "class DelegatingList<T> {\n\
                 \x20 void add(T value) {}\n\
                 }\n";
    let main = "import 'other.dart';\n\
                class Wrong extends DelegatingList<int> {\n\
                \x20 void add(String value) {}\n\
                }\n";
    let names = code_names(&report_two_file(main, other));
    assert!(
        names.contains(&"invalid_override"),
        "add(String) does not override DelegatingList<int>.add: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// #2366 pins: the inherited-signature free-parameter walk. The face walk
// above instantiates the super face's written arguments; the member walk
// must also reach the free container parameters nested inside the member's
// own signature types (`void blast(Wrapper<T> w)` — `T` hides inside
// `Wrapper<T>`). Missing them kept the signature unsubstituted and the
// comparison undecidable (the under-report direction): a broken override in
// the nested form stayed silent where the oracle reports `invalid_override`.
// ---------------------------------------------------------------------------

/// The nested parameter-position form the bare-head walk missed: `Box<T>`'s
/// `blast(Wrapper<T> w)` inherited through `Sub<X> extends Box<X>`
/// instantiates to `blast(Wrapper<X> w)` — both the parameter and the
/// return position (`Wrapper<T> get first`) substitute, so the matching
/// overrides compare equal instead of undecidable. Green at baseline by
/// construction: the undecidable read suppressed rather than false-fired,
/// so the substituted equality is observable only through the mismatch
/// discriminator below.
#[test]
fn nested_member_parameter_position_substitutes_the_free_param() {
    assert_clean(
        "class Wrapper<T> {}\n\
         class Box<T> {\n\
         \x20 void blast(Wrapper<T> w) {}\n\
         \x20 Wrapper<T> get first => throw '';\n\
         }\n\
         class Sub<X> extends Box<X> {\n\
         \x20 void blast(Wrapper<X> w) {}\n\
         \x20 Wrapper<X> get first => throw '';\n\
         }\n",
    );
}

/// The discriminating nested form: `Sub extends Box<int>` instantiates the
/// inherited `blast(Wrapper<T> w)` to `blast(Wrapper<int> w)`, so the
/// `Wrapper<String>` override concretely mismatches and the oracle reports
/// `invalid_override`. The unsubstituted read compared `Wrapper<String>`
/// against the raw `Wrapper<T>` and the free-parameter argument fell
/// undecidable: silent, the under-report this fix closes (red before,
/// green after). The mismatch is concrete (`String` vs `int`) so the
/// verdict is decidable once `T` substitutes — a type-parameter argument
/// (`Wrapper<X>` vs `Wrapper<int>`) stays undecidable in the name-level
/// assignability and cannot discriminate.
#[test]
fn nested_member_parameter_position_reports_the_real_mismatch() {
    let names = code_names(&report(
        "class Wrapper<T> {}\n\
         class Box<T> {\n\
         \x20 void blast(Wrapper<T> w) {}\n\
         }\n\
         class Sub extends Box<int> {\n\
         \x20 void blast(Wrapper<String> w) {}\n\
         }\n",
    ));
    assert!(
        names.contains(&"invalid_override"),
        "blast(Wrapper<String>) does not override Box<int>.blast instantiated T to int: {names:?}"
    );
}

/// The multi-level nested form: `T` sits two argument levels deep in
/// `Outer<Inner<T>>` — the walk descends through both argument lists, so
/// the matching override substitutes equal in the parameter and the
/// return position. Green at baseline by construction, like the
/// single-level clean form (suppression, not false positive); the
/// substituted equality is observable through the mismatch discriminator
/// below.
#[test]
fn deeply_nested_member_position_substitutes_the_free_param() {
    assert_clean(
        "class Inner<A> {}\n\
         class Outer<B> {}\n\
         class Box<T> {\n\
         \x20 void blast(Outer<Inner<T>> w) {}\n\
         \x20 Outer<Inner<T>> get first => throw '';\n\
         }\n\
         class Sub<X> extends Box<X> {\n\
         \x20 void blast(Outer<Inner<X>> w) {}\n\
         \x20 Outer<Inner<X>> get first => throw '';\n\
         }\n",
    );
}

/// The discriminating multi-level nested form: `Sub extends Box<int>`
/// instantiates the inherited `Outer<Inner<T>>` to `Outer<Inner<int>>`, so
/// the `Outer<Inner<String>>` override concretely mismatches two argument
/// levels deep — the same under-report face as the single-level mismatch
/// (red before, green after; the concrete `Inner<String>` vs `Inner<int>`
/// argument decides where a parameter argument would fall undecidable).
#[test]
fn deeply_nested_member_position_reports_the_real_mismatch() {
    let names = code_names(&report(
        "class Inner<A> {}\n\
         class Outer<B> {}\n\
         class Box<T> {\n\
         \x20 void blast(Outer<Inner<T>> w) {}\n\
         }\n\
         class Sub extends Box<int> {\n\
         \x20 void blast(Outer<Inner<String>> w) {}\n\
         }\n",
    ));
    assert!(
        names.contains(&"invalid_override"),
        "blast(Outer<Inner<String>>) does not override Box<int>.blast instantiated T to int: {names:?}"
    );
}

/// The bare-head control the walk always collected: with `T` both as a
/// bare head and nested in one signature, the matching override stays
/// clean while the concrete nested mismatch still reports — the descent
/// widens the set without disturbing the bare-head substitution. Green at
/// baseline by construction: the bare `T head` already fed the
/// substitution, and the shared environment substitutes the nested slot
/// along with it — this pin guards the no-regression face, not the
/// collection gap.
#[test]
fn bare_head_position_control_keeps_substituting() {
    let rep = report(
        "class Wrapper<T> {}\n\
         class Box<T> {\n\
         \x20 void both(T head, Wrapper<T> nested) {}\n\
         }\n\
         class Good<X> extends Box<X> {\n\
         \x20 void both(X head, Wrapper<X> nested) {}\n\
         }\n\
         class Bad extends Box<int> {\n\
         \x20 void both(int head, Wrapper<String> nested) {}\n\
         }\n",
    );
    let overrides: Vec<_> = rep
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .collect();
    assert_eq!(
        overrides.len(),
        1,
        "only Bad's mismatch may report: {:?}",
        rep.diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
}
