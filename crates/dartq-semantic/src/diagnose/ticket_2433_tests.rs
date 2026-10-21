//! The #2433 pins (LAW A phase A3): a member read through a parameterized
//! receiver substitutes the receiver's WRITTEN type arguments into the
//! member's declared face — `Box<int> b; b.set(v)` checks `v` against
//! `int`, never the `T` the member declares. Zero inference: the
//! environment is `(declared parameter -> written argument)` keyed by the
//! receiver's own head, so a bare head writes nothing and stays on the
//! declared face, and a nested instantiation substitutes one level deep.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library — the same
/// minimal rig `ticket_2426_tests` carries.
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

/// Pin (a), the report direction: `Box<int> b; b.set("s")` checks the
/// `String` argument against the instantiated `int` formal — the oracle's
/// `argument_type_not_assignable` row appears exactly where the written
/// instantiation decides it.
#[test]
fn written_args_formal_reports_the_type_mismatch() {
    let source = "class Box<T> { void set(T v) {} } \
                  void use(Box<int> b) { b.set(\"s\"); }";
    let rep = report(source);
    assert!(
        code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// Pin (a), the absolve direction: `Box<int> b; b.set(1)` stays silent —
/// the `int` argument assigns into the instantiated `int` formal, and no
/// family reads the raw `T` face as a mismatch.
#[test]
fn written_args_formal_absolves_the_matching_argument() {
    let source = "class Box<T> { void set(T v) {} } \
                  void use(Box<int> b) { b.set(1); }";
    let rep = report(source);
    assert!(
        !code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// Two-parameter receiver: each written argument lands on its own
/// declared parameter — `Pair<int, String> p; p.put(\"s\", 1)` mismatches
/// both slots and the oracle's row reports, while `p.put(1, \"s\")`
/// assigns into `int`/`String` and stays silent.
#[test]
fn written_args_zip_per_parameter() {
    let bad = "class Pair<A, B> { void put(A a, B b) {} } \
               void use(Pair<int, String> p) { p.put(\"s\", 1); }";
    let rep = report(bad);
    assert!(
        code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
    let good = "class Pair<A, B> { void put(A a, B b) {} } \
                void use(Pair<int, String> p) { p.put(1, \"s\"); }";
    let rep = report(good);
    assert!(
        !code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// Pin (b): a bare-head receiver (`Box b`) writes no arguments, so the
/// environment never composes and the formal keeps the declared `T` face —
/// the unbound-formal suppression stays exactly where it was (no
/// speculative inference, no new row).
#[test]
fn bare_head_receiver_keeps_the_declared_face() {
    let source = "class Box<T> { void set(T v) {} } \
                  void use(Box b) { b.set(\"s\"); }";
    let rep = report(source);
    assert!(
        !code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// Pin (c): the substitution descends one level into the argument list —
/// `Box<Box<int>> b` hands `set` a `Box<int>` formal, so a `Box<String>`
/// argument mismatches argument-wise and reports, while a `Box<int>`
/// argument assigns and stays silent.
#[test]
fn nested_instantiation_substitutes_one_level() {
    let bad = "class Box<T> { void set(T v) {} } \
               void use(Box<Box<int>> b, Box<String> s) { b.set(s); }";
    let rep = report(bad);
    assert!(
        code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
    let good = "class Box<T> { void set(T v) {} } \
                void use(Box<Box<int>> b, Box<int> s) { b.set(s); }";
    let rep = report(good);
    assert!(
        !code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// A member's own type parameter is bound in its signature and never
/// substitutes — `Box<int>.m<M>`'s formal spells the method-level `M`, so
/// the receiver's `int` environment cannot lend `set`'s answer to `m`'s
/// call (the oracle's inference also binds `M := String` and stays
/// silent — either way no row reports).
#[test]
fn member_own_type_params_never_substitute() {
    let source = "class Box<T> { void m<M>(M v) {} } \
                  void use(Box<int> b) { b.m(\"s\"); }";
    let rep = report(source);
    assert!(
        !code_names(&rep).contains(&"argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}
