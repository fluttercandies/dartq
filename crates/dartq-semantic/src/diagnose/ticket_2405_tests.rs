//! The #2405 differential pins: `ambiguous_set_or_map_literal_both`
//! over-reported on the real-package corpus (html `trie.dart` alone ~2.1k
//! rows). The oracle resolves a brace literal by three legs in
//! `typed_literal_resolver.dart` `_computeSetOrMapResolution` — written
//! type arguments, the context type, and the leaf elements — and reports
//! `_both` only when the literal is STILL ambiguous AND its own leaves
//! force opposite sides (`mustBeAMap && mustBeASet`: at least one entry
//! AND at least one bare expression, upstream
//! `pkg/analyzer/lib/src/dart/resolver/typed_literal_resolver.dart:632`).
//!
//! The bug: the byte-level leaf split
//! (`top_level_comma_segments(.., angles=false)`) did not fold a nested
//! literal's `<...>` type-argument group into bracket depth, so an entry
//! value of shape `key: <int, dynamic>{}` split at the group comma into
//! `key: <int` (colon → Map) plus `dynamic>{}` (no colon → Set) — a
//! phantom Set leaf on an all-entry literal, flipping the verdict to
//! `_both`. Each fixture below was probed against oracle `dart analyze`
//! (fvm default).

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

/// The reproduction: an entry whose value is a typed `<int, dynamic>{}`
/// literal. The group's comma must not split the leaf — the all-entry
/// outer literal decides map silently (probed: oracle reports zero rows).
#[test]
fn typed_literal_entry_value_does_not_split_the_leaf() {
    let src = "var m = {59: <int, dynamic>{}};\n";
    let rep = lit_report(src);
    assert!(
        rep.diagnostics.is_empty(),
        "{src:?}: {:?}",
        code_names(&rep)
    );
}

/// The html shape verbatim: nested all-entry literals whose values carry
/// `<int, dynamic>{}` typed-literal bodies (probed: zero rows).
#[test]
fn nested_map_with_typed_literal_values_stays_silent() {
    let src = "var m = {\n\
               \x20 116: {\n\
               \x20   59: <int, dynamic>{},\n\
               \x20   118: {59: <int, dynamic>{}}\n\
               \x20 }\n\
               };\n";
    let rep = lit_report(src);
    assert!(
        rep.diagnostics.is_empty(),
        "{src:?}: {:?}",
        code_names(&rep)
    );
}

/// A comment's own `:`/`,` bytes are not elements: the meta package's
/// `@Target({ // TODO(pq): …
/// TargetKind.getter, … })` annotation is an all-expression set (probed:
/// zero rows), where the `:` inside the comment had split the first leaf
/// into a phantom map entry.
#[test]
fn comment_bytes_do_not_form_map_entries() {
    let src = "@Target({\n\
               \x20 // NOTE: restrict to instance members only\n\
               \x20 TargetKind.getter,\n\
               \x20 TargetKind.setter,\n\
               })\n\
               class Redeclare {}\n\
               const Target({List<Object> members = const []}) ;\n";
    let rep = lit_report(src);
    assert!(
        !code_names(&rep)
            .iter()
            .any(|c| c.starts_with("ambiguous_set_or_map_literal")),
        "{src:?}: {:?}",
        code_names(&rep)
    );
}

/// A genuinely ambiguous mix still reports `_both`: an entry AND a bare
/// expression force opposite sides (probed oracle row).
#[test]
fn entry_plus_expression_still_reports_both() {
    let src = "var m = {1: 2, 3};\n";
    let rep = lit_report(src);
    assert_eq!(
        code_names(&rep),
        ["ambiguous_set_or_map_literal_both"],
        "{:?}",
        code_names(&rep)
    );
}

/// A bare `<`/`>` comparison pair inside an element list is NOT a type
/// group: `{a < b, c > (d)}` reads as expression set leaves, not a folded
/// angle group that swallows the comma. Probed: the comma-separated
/// comparisons are a set → silent on the ambiguity face.
#[test]
fn comparison_operators_do_not_fold_into_angle_depth() {
    let src =
        "void f(bool x, bool y) { var s = {x ? 1 < 2 : 3, y ? 4 > 5 : 6}; }\n";
    let rep = lit_report(src);
    // No ambiguity face may fire; conditional expressions decide set.
    assert!(
        !code_names(&rep)
            .iter()
            .any(|c| c.starts_with("ambiguous_set_or_map_literal")),
        "{src:?}: {:?}",
        code_names(&rep)
    );
}
