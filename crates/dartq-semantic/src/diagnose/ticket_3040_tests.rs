//! The #3040 pins: the unnamed-extension member edge's "no container"
//! sentinel. The extension-application latch mints the reference edge with
//! `face.name.unwrap_or(StringId::NONE)` as its declaring container — an
//! unnamed extension has no name to name — so the `member_refs` column
//! carries the packed-row sentinel. The container-scoped joins downstream
//! (`check_required_callee`'s owner spelling, the deprecated reporter's
//! constructor fallback) must decline sentinel rows instead of resolving
//! them: a resolve of an id the table never issued panics ("StringId not
//! issued by this table"), which is how #3040 crashed the conformance run
//! on `lints/discarded_futures/DiscardedFuturesTest/test_cascadeSection#0`
//! (`var _ = 0..g();` over `extension on int`).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The single-file rig (`ticket_2618_tests`' shape): one library, no
/// imports. The crasher's passes run inside the fold — the pin is that the
/// fold returns at all.
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
    let mut folded = fold.report;
    folded.diagnostics = merge_source_ordered(fold.parse_diagnostics, folded.diagnostics);
    (folded, strings)
}

/// The crasher's shape: an unnamed extension's member invoked through an
/// implicit application (here a cascade on a fresh receiver — the
/// conformance case's `0..g()` with an in-file `on` type so the rig's
/// resolution stands). The fold must complete, the edge must carry the
/// sentinel as its container, and the required-named check must decline
/// the row — the collector records no `formal_annotations` owner for a
/// member of a container with no name, so there is nothing to join and no
/// `missing_required_param` may fire off the bare member name.
#[test]
fn unnamed_extension_member_edge_carries_the_sentinel_container() {
    let source = "class Host {}\n\
                  extension on Host {\n\
                  void g() {}\n\
                  }\n\
                  void f() {\n\
                  var _ = Host()..g();\n\
                  }\n";
    let (folded, strings) = report(source);
    let edges: Vec<(&str, bool)> = folded
        .member_refs
        .iter()
        .map(|edge| {
            (
                strings.resolve(edge.target_name),
                edge.target_container == StringId::NONE,
            )
        })
        .collect();
    assert_eq!(
        edges,
        vec![("g", true)],
        "the unnamed extension's invoked member edges under the sentinel"
    );
    assert!(
        !folded
            .diagnostics
            .iter()
            .any(|diag| diag.code.name.contains("missing_required_param")),
        "a sentinel row declines the required-named join"
    );
}

/// The named-extension twin: a static access through a named extension
/// composes the real `Ext.stat` owner and the fold completes — the guard
/// narrows to the sentinel, never to extension edges at large.
#[test]
fn named_extension_member_edge_composes_the_real_container() {
    let source = "class Host {}\n\
                  extension Ext on Host {\n\
                  static void stat() {}\n\
                  }\n\
                  void f() {\n\
                  Ext.stat();\n\
                  }\n";
    let (folded, strings) = report(source);
    let edges: Vec<(&str, String)> = folded
        .member_refs
        .iter()
        .map(|edge| {
            let spelling = strings.resolve(edge.target_name);
            (
                spelling.strip_suffix('=').unwrap_or(spelling),
                strings.resolve(edge.target_container).to_owned(),
            )
        })
        .collect();
    assert_eq!(
        edges,
        vec![("stat", "Ext".to_owned())],
        "the named extension's static edge resolves its container"
    );
}
