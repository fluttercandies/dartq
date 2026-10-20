//! #2318 pins: the extension-member element-identity credit. The
//! `unused_element` family spares an extension member only when a
//! resolved implicit application bound THAT member's element — the
//! oracle's `usedElements.members` set, keyed by element identity
//! (upstream Dart SDK source, pkg/analyzer
//! `unused_local_elements_verifier.dart` `_isUsedMember`: no spelling
//! arm, and `_useIdentifierElement`'s `identical(element.library)` gate
//! keeps the credit library-local).
//!
//! Before the fix the credit was a per-library NAME set, so an
//! unresolved access spelling (`Receiver::Unknown` keeps `found = true`)
//! released a staged member row at the library close. The corpus face
//! is the analyzer package itself: 19 `UNUSED_ELEMENT` rows dartq
//! missed across both-broken files, every one a member of an unnamed or
//! private-named extension.
//!
//! Oracle provenance: each expectation was probed with `dart analyze`
//! over the same sources during #2188/#2318 (`/tmp/probe2318` family).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of an import-free library — the
/// `tests.rs` `report` rig mirrored (the sibling rig stays off-limits
/// while its lane is in flight).
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

/// An unnamed extension's unused member still reports — the shape the
/// corpus residual depends on (`extension on BinaryReader { ... }`).
#[test]
fn unnamed_extension_unused_member_reports() {
    let source = "class L {}\nextension on L { int get unused => 0; }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"unused_element"), "{names:?}");
}

/// Two unnamed extensions declaring the SAME spelling: an application
/// that binds the first spares only that face — the second's member
/// stays unused. The name-level credit spared both.
#[test]
fn same_spelling_across_unnamed_extensions_stays_isolated() {
    let source = "class A {}\n\
                  class B {}\n\
                  extension on A { int get g => 0; }\n\
                  extension on B { int get g => 1; }\n\
                  void f(A a) { a.g; }\n";
    let names = code_names(&report(source));
    let unused = names.iter().filter(|c| **c == "unused_element").count();
    assert_eq!(unused, 1, "{names:?}");
}

/// The twin across a private-named extension and an unnamed one — the
/// element key spans both face shapes.
#[test]
fn named_and_unnamed_extension_same_spelling_stays_isolated() {
    let source = "class A {}\n\
                  class B {}\n\
                  extension _E on A { int get g => 0; }\n\
                  extension on B { int get g => 1; }\n\
                  void f(B b) { b.g; }\n";
    let names = code_names(&report(source));
    let unused = names.iter().filter(|c| **c == "unused_element").count();
    assert_eq!(unused, 1, "{names:?}");
}

/// A member read through an UNRESOLVED receiver never credits: the
/// oracle's `usedElements.members` only gains on a resolved access, so
/// `unknown.g` (receiver of unknown static type) still leaves `g`
/// unused on every face.
#[test]
fn unresolved_receiver_read_never_spares_an_extension_member() {
    let source = "class L {}\n\
                  extension on L { int get g => 0; }\n\
                  void f(dynamic d) { d.g; }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"unused_element"), "{names:?}");
}

/// The positive control: a resolved application spares its own member
/// and leaves its sibling reported — one row, not zero and not two.
#[test]
fn resolved_application_spares_exactly_its_member() {
    let source = "class L {}\n\
                  extension on L { int get used => 0; int get unused => 1; }\n\
                  void f(L l) { l.used; }\n";
    let names = code_names(&report(source));
    let unused = names.iter().filter(|c| **c == "unused_element").count();
    assert_eq!(unused, 1, "{names:?}");
}

/// A static extension member resolves through the container-static path
/// (`_E._x`), not an implicit application: the spelling arms keep
/// sparing it, and the member-level gate must not report it as unused.
#[test]
fn static_extension_member_keeps_its_spelling_credit() {
    let source = "class L {}\n\
                  extension _E on L { static int _s = 0; }\n\
                  void f() { _E._s; }\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"unused_element"), "{names:?}");
}
