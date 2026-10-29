//! The #2245 pins: the composite-argument-window faces verified
//! end-to-end through the single-file rig.
//!
//! A comparison argument (`a == b`, `a != b`) folds no single row — the
//! `==`/`!=`/`&&`/`||` spellings carry no `mark_operator` mark, so
//! `fold_binary` keeps the two operand rows — and the argument windows
//! must still attribute it: the statically-`bool` composite face
//! (`composite_bool_arg`, answered by `latch_bool_span`'s extents)
//! recovers `bool` at the composite's own extent, and a multi-row window
//! never vanishes (the former `flush_unclaimed` drop shifted every later
//! argument onto the wrong formal — the intl `_checkAndThrow`
//! misalignment against the oracle).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (mirrored from the #2216 module; the shared
/// `tests` rig stays untouched).
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

fn arg_type_rows(report: &FileReport) -> Vec<(u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect()
}

/// The intl `_checkAndThrow` shape: a multiline argument list whose
/// first argument is a null comparison. The comparison is `bool` against
/// the `bool` formal — the oracle is silent, and the string must pair
/// with the `String` formal, never the comparison's.
#[test]
fn comparison_first_argument_pairs_with_its_own_formal() {
    let source = "\
void checkAndThrow(bool condition, String message) {}

void f(Object? option) {
  checkAndThrow(
    option != null,
    'Invalid option or missing other option for select block.',
  );
}
";
    let rows = arg_type_rows(&report(source));
    assert!(rows.is_empty(), "{rows:?}");
}

/// The negative anchor: against an `int` first formal the comparison
/// reports `bool` -> `int` at the composite's own extent, and the second
/// argument still pairs with the second formal (no row for the string).
#[test]
fn comparison_argument_reports_at_its_own_extent() {
    let source = "\
void g(int n, String s) {}

void f(Object? option) {
  g(
    option != null,
    'x',
  );
}
";
    let report = report(source);
    let rows = arg_type_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (offset, message) = &rows[0];
    let composite = source.find("option != null").unwrap() as u32;
    assert_eq!(*offset, composite, "anchor at the composite extent");
    assert!(message.contains("'bool'"), "{message:?}");
    assert!(message.contains("'int'"), "{message:?}");
}

/// The named window: a comparison as a named argument's value pairs
/// with the same-spelled formal and reports at the composite extent.
#[test]
fn named_comparison_argument_pairs_with_named_formal() {
    let source = "\
void h({required int n, required String s}) {}

void f(Object? option) {
  h(
    n: option != null,
    s: 'x',
  );
}
";
    let report = report(source);
    let rows = arg_type_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (offset, message) = &rows[0];
    let composite = source.find("option != null").unwrap() as u32;
    assert_eq!(*offset, composite, "anchor at the composite extent");
    assert!(message.contains("'bool'"), "{message:?}");
    assert!(message.contains("'int'"), "{message:?}");
}

/// The last positional argument rides `flush_argument`'s close path: a
/// trailing comparison lands on its own formal (the unknown face stayed
/// silent where the oracle reports).
#[test]
fn trailing_comparison_argument_lands_on_its_own_formal() {
    let source = "\
void k(String s, int n) {}

void f(Object? option) {
  k('x', option != null);
}
";
    let report = report(source);
    let rows = arg_type_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (offset, message) = &rows[0];
    let composite = source.find("option != null").unwrap() as u32;
    assert_eq!(*offset, composite, "anchor at the composite extent");
    assert!(message.contains("'bool'"), "{message:?}");
    assert!(message.contains("'int'"), "{message:?}");
}

/// The lazy-boolean composite over two comparisons: the outermost
/// `&&` extent covers the whole window — one `bool` argument, silent
/// against the `bool` formal.
#[test]
fn lazy_and_composite_argument_stays_bool() {
    let source = "\
void m(bool c, String s) {}

void f(Object? a, Object? b) {
  m(
    a != null && b != null,
    'x',
  );
}
";
    let rows = arg_type_rows(&report(source));
    assert!(rows.is_empty(), "{rows:?}");
}
