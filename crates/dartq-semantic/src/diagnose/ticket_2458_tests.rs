//! The #2458 differential pins: a comment between the last element and
//! the closing `}` of a context-less set-or-map literal counted as a
//! phantom element in the byte-level leaf census — a comment-only
//! segment survived the leading-trim and classified as a bare
//! expression (no top-level colon → Set side), so an all-entry literal
//! flipped to `ambiguous_set_or_map_literal_both` (intl
//! `date_time_patterns.dart` alone carried 122 such rows). Upstream
//! reads elements from the AST, where comments do not exist: every
//! fixture below was probed silent against oracle `dart analyze` (fvm
//! default) unless stated otherwise.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module.
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

/// The collection-head stubs the written annotations name
/// (ticket_2326 harness convention).
const LITERAL_HARNESS: &str = "class int {}\nclass String {}\nclass List<T> {}\n\
                              class Set<T> {}\nclass Map<K, V> {}\nclass Iterable<T> {}\n\
                              class Stream<T> {}\nclass dynamic_ {}\nclass Object {}\n";

fn lit_report(source: &str) -> FileReport {
    report(&(String::from(LITERAL_HARNESS) + source))
}

fn assert_no_ambiguity(source: &str) {
    let rep = lit_report(source);
    assert!(
        !code_names(&rep)
            .iter()
            .any(|c| c.starts_with("ambiguous_set_or_map_literal")),
        "{source:?}: {:?}",
        code_names(&rep)
    );
}

/// The intl `date_time_patterns.dart` shape: a same-line `//` comment
/// trailing the inner literal's last entry (probed: zero rows).
#[test]
fn same_line_comment_after_last_entry_stays_silent() {
    let src = "var x = {'a': {'b': 'c', // cm\n}};\n";
    assert_no_ambiguity(src);
}

/// The same trailing comment on its own line, `//` form (probed: zero
/// rows).
#[test]
fn own_line_comment_after_last_entry_stays_silent() {
    let src = "var y = {\n  'a': const {\n    'b': 'c',\n    // cm\n  },\n};\n";
    assert_no_ambiguity(src);
}

/// The doc-comment (`///`) form of the trailing comment (probed: zero
/// rows).
#[test]
fn doc_comment_after_last_entry_stays_silent() {
    let src = "var d = {\n  'a': 1,\n  /// doc cm\n};\n";
    assert_no_ambiguity(src);
}

/// The block-comment form of the trailing comment (probed: zero rows).
#[test]
fn block_comment_after_last_entry_stays_silent() {
    let src = "var b = {\n  'a': 1,\n  /* blk */\n};\n";
    assert_no_ambiguity(src);
}

/// A trailing comma ahead of the trailing comment leaves the segment
/// comment-only all the same (probed: zero rows).
#[test]
fn comment_after_trailing_comma_stays_silent() {
    let src = "var t = {\n  'a': 1, // cm\n};\n";
    assert_no_ambiguity(src);
}

/// Comments before the first element and between elements were never
/// elements — boundary guard for the trivia-aware trim (probed: zero
/// rows).
#[test]
fn leading_and_interior_comments_stay_silent() {
    let src = "var f = { // lead\n  'a': 1,\n};\nvar g = {\n  'a': 1, /* mid */ 'b': 2,\n};\n";
    assert_no_ambiguity(src);
}

/// An elementless literal stays silent whatever trivia it carries — the
/// elementless form defaults to a map and reports nothing (probed: zero
/// rows for both `{ /* c */ }` and a line-comment body).
#[test]
fn comment_only_empty_literal_stays_silent() {
    let src = "var e1 = {/* c */};\nvar e2 = { // c\n};\n";
    assert_no_ambiguity(src);
}

/// A genuinely mixed literal still reports `_both` with a trailing
/// comment — trivia removal must not mute a real verdict (probed: the
/// oracle reports `_both` on both fixtures).
#[test]
fn genuine_mix_with_trailing_comment_still_reports_both() {
    let src = "var m = {\n  1,\n  2,\n  'a': 3, // cm\n};\n";
    let rep = lit_report(src);
    assert_eq!(
        code_names(&rep),
        ["ambiguous_set_or_map_literal_both"],
        "{src:?}: {:?}",
        code_names(&rep)
    );
}
