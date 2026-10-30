//! The #2934 pins: a nameless declaration CONTAINER (parser recovery —
//! `class {}`, `enum { e1 }`, `mixin {}`, `extension type(int i) {}`) is
//! PUBLIC upstream, so `unused_element` (a private-only code) never fires
//! on it. `FragmentImpl.isPrivate` maps a null name to PUBLIC
//! (upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
//! element.dart:4539-4544); the element's `displayName` renders
//! `name ?? '<unnamed>'`, but the privacy verdict still reads the null
//! name — not the rendered text. dartq over-reported `<unnamed>` rows
//! when the recovered empty-spelling site leaked into a private-or-
//! nameless eligible gate.
//!
//! The regression arm pins the opposite direction: a private-named
//! container with no references still reports `unused_element`.

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

fn unused_element_rows(report: &FileReport) -> Vec<(u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect()
}

/// The nameless class container reports no `unused_element`
/// (element.dart:4539-4544 — a null fragment name resolves PUBLIC).
/// The oracle's only rows on this source are the parser's own
/// `Expected an identifier.` recovery face, not the usage cluster's.
#[test]
fn nameless_class_container_is_public_not_unused() {
    let report = unit_report("library;\n\nclass {}\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// A nameless class with a member body (`class { void f() {} }`,
/// the `test_declarations_class_unnamed` /
/// `test_error_undefinedMethod_noTarget_synthetic_class` corpus shape):
/// neither the container nor its public member reports — the member's
/// enclosing interface element is public (null fragment name →
/// `isPrivate` false), so the accessibility walk spares it.
#[test]
fn nameless_class_with_member_reports_nothing() {
    let report = unit_report("class {\n  void f() {\n    foo(0);\n  }\n}\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// A named container followed by a nameless one
/// (`test_declarations_cancel`'s shape): the nameless container's own
/// `1:1` row must not appear, and the named public class stays spared.
#[test]
fn nameless_container_after_named_reports_nothing() {
    let report = unit_report("class A {}\nclass {\n  void m() {}\n}\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The nameless enum container (`enum { e1 }`) — same null-name → PUBLIC
/// verdict; the enum constants ride their own declaration face.
#[test]
fn nameless_enum_container_is_public_not_unused() {
    let report = unit_report("library;\n\nenum { e1 }\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The nameless mixin container (`mixin {}`).
#[test]
fn nameless_mixin_container_is_public_not_unused() {
    let report = unit_report("library;\n\nmixin {}\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The nameless extension-type container (`extension type(int i) {}`).
#[test]
fn nameless_extension_type_container_is_public_not_unused() {
    let report = unit_report("library;\n\nextension type(int i) {}\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The nameless class-type-alias container (`class = Object with M;`).
#[test]
fn nameless_class_type_alias_container_is_public_not_unused() {
    let report = unit_report("library;\n\nmixin M {}\nclass = Object with M;\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The nameless enum's constant set (`enum { foo }`): the container's
/// own `<unnamed>` row is suppressed and the public-named constant
/// rides its own accessibility (its enclosing interface element is
/// public) — oracle reports neither.
#[test]
fn nameless_enum_constant_is_public_not_unused() {
    let report = unit_report("library;\n\nenum { foo }\n");
    let rows = unused_element_rows(&report);
    assert!(rows.is_empty(), "{rows:?}");
}

/// Regression: a private-named container nothing references still
/// reports `unused_element` — the suppression must not swallow the
/// private gate itself.
#[test]
fn private_named_unused_container_still_reports() {
    let report = unit_report("library;\n\nclass _C {}\n");
    let rows = unused_element_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].1.contains("_C"), "{rows:?}");
}

/// Regression: a private-named top-level function nothing calls still
/// reports `unused_element`.
#[test]
fn private_named_unused_function_still_reports() {
    let report = unit_report("library;\n\nvoid _f() {}\n");
    let rows = unused_element_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].1.contains("_f"), "{rows:?}");
}
