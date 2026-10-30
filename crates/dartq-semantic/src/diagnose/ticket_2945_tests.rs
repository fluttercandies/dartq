//! The #2945 pins: the constructor-use `member_refs` edge is gated on the
//! declaration's own metadata. #2618's zero-edge contract held for named
//! constructors because the edge exists only to carry the ctor's
//! annotations into `check_target_annotations` — a constructor whose
//! `ctor_annotations` star is empty gives the verifier nothing to read, and
//! the row was leaking into the index `RefRow` surface as a spurious
//! member reference. #2945 narrows `note_ctor_use_in` so the edge is
//! published only when the resolved constructor's declaration carries
//! annotations.
//!
//! Upstream firing condition (unchanged): `ElementUsageDetector.
//! constructorName` reads `node.element`'s metadata —
//! pkg/analyzer/lib/src/error/element_usage_detector.dart:189-191, driven
//! from `best_practices_verifier.dart` `visitConstructorName`:321-324.
//! The instantiation itself is still a real use for the oracle's
//! element-usage face — the creation head's `Referenced`/`type_uses` rows
//! keep marking the class — so a bare `Host()` still credits `Host` while
//! no member-shaped index row is minted.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The single-file rig (`ticket_2618_tests`' shape): one library, no
/// imports — enough to assert on `member_refs` itself. The edge gate reads
/// the declaration's `ctor_annotations` star, which same-file annotations
/// populate exactly like cross-package ones.
fn report(source: &str) -> (FileReport, StringTable) {
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
    (report, strings)
}

/// The member-ref rows as `(offset, kind, target spelling)` triples. A
/// ctor edge's `range` covers the whole `ConstructorName` (the `Host.named`
/// compound for a named creation, the `Host` head for a bare one), so —
/// unlike `ticket_2618_tests::rows`' member-token assertion — the offset
/// is NOT the selector token; the projection checks `target_container`
/// names the class and reports `target_name` as the selector spelling.
fn rows(
    source: &str,
    report: &FileReport,
    strings: &StringTable,
) -> Vec<(usize, &'static str, String)> {
    let mut rows: Vec<_> = report
        .member_refs
        .iter()
        .map(|row| {
            let spelling = strings.resolve(row.target_name);
            let container = strings.resolve(row.target_container);
            let offset = row.range.offset as usize;
            // The compound anchor: the class head leads the written span.
            assert!(
                source[offset..].starts_with(container),
                "the row at {offset} starts `{container}`: {}",
                &source[offset.min(source.len())..(offset + 16).min(source.len())]
            );
            (
                offset,
                match row.kind {
                    MemberRefKind::Read => "read",
                    MemberRefKind::Invoked => "invoked",
                    MemberRefKind::Written => "written",
                    MemberRefKind::ReadWritten => "read_written",
                },
                spelling.to_owned(),
            )
        })
        .collect();
    rows.sort_by_key(|&(offset, _, _)| offset);
    rows
}

/// A bare `Host()` on an unannotated constructor mints no member edge —
/// the #2618 zero restored: nothing on the declaration for the verifier to
/// read, so the index surface stays clean.
#[test]
fn bare_constructor_call_mints_no_member_edge() {
    let source = "class Host {\n\
                  Host();\n\
                  }\n\
                  void f() {\n\
                  Host();\n\
                  }\n";
    let (report, strings) = report(source);
    assert!(
        rows(source, &report, &strings).is_empty(),
        "the bare unannotated ctor call keeps #2618's zero edge: {report:?}"
    );
}

/// A named-ctor call on an unannotated constructor mints no member edge —
/// the selector has no `ctor_annotations` star to carry.
#[test]
fn unannotated_named_constructor_mints_no_member_edge() {
    let source = "class Host {\n\
                  Host.named();\n\
                  }\n\
                  void f() {\n\
                  Host.named();\n\
                  }\n";
    let (report, strings) = report(source);
    assert!(
        rows(source, &report, &strings).is_empty(),
        "no annotations on the declaration, no edge: {report:?}"
    );
}

/// An annotated ctor still publishes the edge — `member: Some` on the
/// verifier side drains `ctor_annotations`, so the edge is exactly what the
/// `deprecated_member_use` / `experimental_member_use` faces need.
#[test]
fn annotated_constructor_keeps_the_member_edge() {
    let source = "class Host {\n\
                  @deprecated Host.named();\n\
                  }\n\
                  void f() {\n\
                  Host.named();\n\
                  }\n";
    let (report, strings) = report(source);
    let got = rows(source, &report, &strings);
    assert_eq!(
        got.len(),
        1,
        "the annotated ctor publishes its carrier edge: {report:?}"
    );
    assert_eq!(got[0].1, "invoked");
    assert_eq!(got[0].2, "named");
}

/// The annotation gate keys on the DECLARATION's star, not the call: a
/// mixed file (annotated `named`, bare `other`) publishes only the edge
/// the verifier can consume.
#[test]
fn only_the_annotated_face_publishes() {
    let source = "class Host {\n\
                  @deprecated Host.named();\n\
                  Host.other();\n\
                  }\n\
                  void f() {\n\
                  Host.named();\n\
                  Host.other();\n\
                  Host();\n\
                  }\n";
    let (report, strings) = report(source);
    let got = rows(source, &report, &strings);
    assert_eq!(got.len(), 1, "exactly the annotated ctor edges: {report:?}");
    assert_eq!(got[0].2, "named");
}
