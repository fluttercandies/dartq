//! The #2340 differential pins: the InvalidType-scrutinee coverage face
//! over the switch dead-case rows — upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart`
//! `promoteForPattern` (flow_analysis.dart:7708-7768): the matched type is
//! covered when the scrutinee's static type subtypes the case's required
//! type, and `InvalidType` subtypes everything vacuously
//! (subtype.dart:274-276), so the first guard-free resolvable-typed case
//! absorbs the match and every following clause reports `dead_code` at its
//! keyword plus its first body statement. A scrutinee whose static type
//! never resolved in the diagnosing scope is that `InvalidType`, while
//! dartq's flow face answers the unknown shape like `dynamic` — whose
//! verdict keeps every case live (the `dynamic` control stays silent on
//! both sides). The twin over-report face (`pattern_never_matches_value_type`
//! / `unreachable_switch_default` on an unresolved *pattern* type — the
//! oracle's Invalid arm joins unmatched without a verdict) is pinned
//! silent beside it. Each fixture shape was probed against oracle
//! `dart analyze` (fvm default) on an isolated broken-import tree before
//! pinning: bare-identifier and `<id>.<id>` scrutinees dead, guarded
//! absorbers and wildcard/constant patterns fail open, `default` after the
//! absorber reports `dead_code` (not `unreachable_switch_default`), empty
//! bodies contribute the head row only.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The two-library rig with a deliberately broken import (the #2336 rig):
/// `dependency` is analyzed and declared into the graph (the world's
/// unique-declaration fallback can see it — the corpus posture), but the
/// main unit's `import 'missing.dart';` carries no graph edge unless
/// `imported`, so the namespace face cannot resolve the dependency's
/// names.
fn report(main_source: &str, dependency: &str, imported: bool) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        main_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {main_source:?}"
    );
    let scanned_dep = dartq_scanner::scan(
        dependency,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dep_file,
    );
    assert!(
        scanned_dep.diagnostics.is_empty(),
        "scanner diagnostics: {dependency:?}"
    );
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let dep_elements = collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern("package:t/kit.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in dep_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(dep_lib, name, dep_file);
    }
    if imported {
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    diagnose_with(
        main_source,
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

fn dead_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

fn codes(report: &FileReport, name: &str) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| d.range.offset)
        .collect()
}

const SEALED_SOURCE: &str = "sealed class Source {}\n\
                             class PathSource extends Source {}\n\
                             class VersionedSource extends Source {}\n\
                             class OtherSource extends Source {}\n";

/// The analyzer corpus' sealed-switch shape: the scrutinee is a member
/// select over a receiver whose declared type never resolved (the broken
/// import keeps `K` out of the namespace), the case patterns resolve
/// in-unit. The oracle absorbs the match at the first case and deads the
/// second case's keyword and body (probed: `DEAD_CODE` at the `case`
/// keyword, length 4, and at the body statement).
#[test]
fn broken_receiver_select_deads_the_following_cases() {
    let source = SEALED_SOURCE.to_string()
        + "void f(K c) {\n\
          \x20 switch (c.source) {\n\
          \x20   case PathSource():\n\
          \x20     'path';\n\
          \x20   case VersionedSource():\n\
          \x20     'ver';\n\
          \x20 }\n\
          }";
    let report = report(&source, "class K {}", false);
    let case_at = source.find("case VersionedSource()").unwrap() as u32;
    let body_at = source.find("'ver'").unwrap() as u32;
    assert_eq!(
        dead_rows(&report),
        vec![(case_at, 4), (body_at, 6)],
        "{:?}",
        report.diagnostics
    );
}

/// The bare-identifier variant: an unresolved declared type on the
/// scrutinee itself decides the same coverage face (probed: the oracle
/// deads the following case keyword and body identically).
#[test]
fn unresolved_scrutinee_type_deads_the_following_cases() {
    let source = SEALED_SOURCE.to_string()
        + "void f(K k) {\n\
          \x20 switch (k) {\n\
          \x20   case PathSource():\n\
          \x20     'path';\n\
          \x20   case VersionedSource():\n\
          \x20     'ver';\n\
          \x20 }\n\
          }";
    let report = report(&source, "class K {}", false);
    let case_at = source.find("case VersionedSource()").unwrap() as u32;
    let body_at = source.find("'ver'").unwrap() as u32;
    assert_eq!(
        dead_rows(&report),
        vec![(case_at, 4), (body_at, 6)],
        "{:?}",
        report.diagnostics
    );
}

/// The control: the same switch over a resolved receiver stays live on
/// both sides — the synthesized face must not leak into the resolved world
/// (the `dynamic`/resolved control probe keeps silent).
#[test]
fn resolved_receiver_keeps_every_case_live() {
    let dep = "sealed class Source {}\n\
               class PathSource extends Source {}\n\
               class VersionedSource extends Source {}\n\
               class Holder { final Source source = PathSource(); }";
    let source = "import 'missing.dart';\n\
                  void f(Holder h) {\n\
                  \x20 switch (h.source) {\n\
                  \x20   case PathSource():\n\
                  \x20     'path';\n\
                  \x20   case VersionedSource():\n\
                  \x20     'ver';\n\
                  \x20 }\n\
                  }";
    let report = report(source, dep, true);
    assert!(dead_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// A `when` guard on the covering case fails the face open: the
/// guard-false arm joins the unmatched state, so the oracle keeps the
/// later clauses live (probed) — synthesizing there would over-report.
#[test]
fn guarded_absorber_fails_open() {
    let source = SEALED_SOURCE.to_string()
        + "void f(K c) {\n\
          \x20 switch (c.source) {\n\
          \x20   case PathSource() when c != null:\n\
          \x20     'path';\n\
          \x20   case VersionedSource():\n\
          \x20     'ver';\n\
          \x20 }\n\
          }";
    let report = report(&source, "class K {}", false);
    assert!(dead_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// A `default` after the absorbing case reports `dead_code` at the
/// `default` keyword itself (length 7) plus its body — not
/// `unreachable_switch_default` (probed against the oracle).
#[test]
fn default_after_the_absorber_deads_at_the_keyword() {
    let source = SEALED_SOURCE.to_string()
        + "void f(K c) {\n\
          \x20 switch (c.source) {\n\
          \x20   case PathSource():\n\
          \x20     'path';\n\
          \x20   default:\n\
          \x20     'def';\n\
          \x20 }\n\
          }";
    let report = report(&source, "class K {}", false);
    let default_at = source.find("default").unwrap() as u32;
    let body_at = source.find("'def'").unwrap() as u32;
    assert_eq!(
        dead_rows(&report),
        vec![(default_at, 7), (body_at, 6)],
        "{:?}",
        report.diagnostics
    );
}

/// Empty bodies keep only their head rows; the first body statement after
/// the last empty case owns the single body row (probed: one `DEAD_CODE`
/// per clause keyword, one for the shared statement).
#[test]
fn empty_bodies_keep_head_rows_only() {
    let source = SEALED_SOURCE.to_string()
        + "void f(K c) {\n\
          \x20 switch (c.source) {\n\
          \x20   case PathSource():\n\
          \x20     'path';\n\
          \x20   case VersionedSource():\n\
          \x20   case OtherSource():\n\
          \x20     'other';\n\
          \x20 }\n\
          }";
    let report = report(&source, "class K {}", false);
    let case2_at = source.find("case VersionedSource()").unwrap() as u32;
    let case3_at = source.find("case OtherSource()").unwrap() as u32;
    let body_at = source.find("'other'").unwrap() as u32;
    assert_eq!(
        dead_rows(&report),
        vec![(case2_at, 4), (case3_at, 4), (body_at, 8)],
        "{:?}",
        report.diagnostics
    );
}

/// The A2 twin: an unresolved *pattern* type never reports — the oracle's
/// Invalid arm joins the unmatched state without a verdict, so neither
/// `pattern_never_matches_value_type` nor `unreachable_switch_default`
/// fires on the invalid posture (probed).
#[test]
fn unresolved_pattern_type_stays_silent() {
    let source = "void f(K c) {\n\
                  \x20 switch (c) {\n\
                  \x20   case GitSource():\n\
                  \x20     'g';\n\
                  \x20   default:\n\
                  \x20     'd';\n\
                  \x20 }\n\
                  }";
    let report = report(source, "class K {}", false);
    assert!(codes(&report, "pattern_never_matches_value_type").is_empty());
    assert!(codes(&report, "unreachable_switch_default").is_empty());
    assert!(dead_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The negative twin: a resolvable disjoint pattern type over a resolved
/// scrutinee keeps the official `pattern_never_matches_value_type` rows —
/// the silence must not leak into the resolved world.
#[test]
fn resolved_disjoint_pattern_keeps_the_never_match_row() {
    let source = "enum E { a }\n\
                  class A {}\n\
                  void f(E e) {\n\
                  \x20 switch (e) {\n\
                  \x20   case A x: x; break;\n\
                  \x20   case E _: break;\n\
                  \x20 }\n\
                  }";
    let report = report(source, "class Unused {}", false);
    assert_eq!(
        codes(&report, "pattern_never_matches_value_type").len(),
        1,
        "{:?}",
        report.diagnostics
    );
}

/// The negative twin for the default face: total coverage over a resolved
/// scrutinee keeps `unreachable_switch_default` at the keyword.
#[test]
fn resolved_total_coverage_keeps_the_unreachable_default() {
    let source = "enum Color { red, green, blue }\n\
                  void f(Color c) {\n\
                  \x20 switch (c) {\n\
                  \x20   case Color.red: break;\n\
                  \x20   case Color.green: break;\n\
                  \x20   case Color.blue: break;\n\
                  \x20   default: break;\n\
                  \x20 }\n\
                  }";
    let report = report(source, "class Unused {}", false);
    assert_eq!(codes(&report, "unreachable_switch_default").len(), 1);
}
