//! The #2328 differential pins: the chain-broken member/on-face climb and
//! the bare-name faces it must not own. The member-face arms of
//! `resolve_bare` claim a bare name only on a clean `Walk::Found` — a
//! `ChainBroken` (a supertype or `on`-type hop that resolves to a
//! declaration the element index cannot model) leaves the member question
//! undecided, so the name falls through to the ordinary scope lookup: a
//! bare top-level creation send inside a broken-chain class resolves there
//! instead of masquerading as an implicit-`this` member (the drift corpus's
//! false `undefined_method` rows), and an annotation name — which upstream
//! resolves like any constant-context expression through the scope alone
//! (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/annotation_resolver.dart`), never
//! through a member or `on`-type face — reports `undefined_annotation` on
//! the scope miss instead of the broken climb's blanket suppression (the
//! petitparser/xml corpus's silent o-only rows: `@useResult` inside an `on
//! String` extension). The twin guards hold both faces: a real member call
//! over a chain-broken receiver keeps reporting, and an annotation name the
//! scope answers stays silent.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The rig mirroring the corpus posture: every dependency is declared into
/// the graph and imported, but only the deps flagged `true` carry an
/// element model — a name from an unmodeled dep resolves "defined" with no
/// walkable members (the synthetic SDK face the corpus's unresolvable
/// packages project), so a supertype or `on`-type spelled with it breaks
/// the member climb while a scope lookup still answers. `dart:core` is
/// seeded implicitly with the unindexed face, like the engine driver does.
fn report(main_source: &str, deps: &[(&str, &str, bool)]) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &["Object", "String", "int", "bool", "double", "num", "Match"],
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    for (path, dep_source, indexed) in deps {
        let dep_file = files.intern(Path::new(path));
        let dep_lib = builder.intern(&format!("package:t/{path}"));
        let scanned_dep = dartq_scanner::scan(
            dep_source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            dep_file,
        );
        assert!(
            scanned_dep.diagnostics.is_empty(),
            "scanner diagnostics: {dep_source:?}"
        );
        let dep_elements = collect(dep_source, &scanned_dep.tokens, features, &strings);
        for (name, decl) in dep_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(dep_lib, name, dep_file);
        }
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        if *indexed {
            index.insert(dep_file, dep_elements);
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
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

fn codes(report: &FileReport, name: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The petitparser/xml corpus face: `@useResult` inside an `extension ... on
/// String` body, with `useResult` defined nowhere (the corpus's
/// `package:meta` import never resolves). The `on`-type climb ends at the
/// unindexed core face — a `ChainBroken` the member-face arm must not read
/// as resolved — so the name falls through to the scoped lookup and the
/// miss reports `undefined_annotation` over the `@name` span (probed
/// against the oracle: `Undefined name 'internal' used as an annotation.`
/// over the same posture).
#[test]
fn extension_annotation_over_broken_on_face_reports() {
    let source = "extension E on String {\n\
                  \x20 @useResult\n\
                  \x20 bool get isLong => length > 9;\n\
                  }";
    let report = report(source, &[]);
    let at = source.find("@useResult").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![(at, "@useResult".len() as u32)],
        "{:?}",
        report.diagnostics
    );
    assert!(
        codes(&report, "undefined_identifier").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The fall-through lands on the scope when the name is defined there: the
/// annotation resolves like any constant-context identifier and stays
/// silent — the broken climb must not suppress a resolvable name, and the
/// scope miss must be the only new face.
#[test]
fn extension_annotation_resolved_in_scope_stays_silent() {
    let source = "const useResult = 0;\n\
                  extension E on String {\n\
                  \x20 @useResult\n\
                  \x20 bool get isLong => length > 9;\n\
                  }";
    let report = report(source, &[]);
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![],
        "{:?}",
        report.diagnostics
    );
    assert!(
        codes(&report, "undefined_identifier").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The class-body twin (the xml corpus rows): an annotation inside a class
/// whose superclass resolves to the unmodeled face — the member climb is
/// broken, the scope lookup misses, and `undefined_annotation` reports (the
/// landed `Found`-only claim face; pinned so the annotation fall-through
/// and the member fall-through hold together).
#[test]
fn class_annotation_over_broken_superclass_reports() {
    let dep = "class Base {}";
    let source = "class C extends Base {\n\
                  \x20 @internal\n\
                  \x20 void m() {}\n\
                  }";
    let report = report(source, &[("dep0.dart", dep, false)]);
    let at = source.find("@internal").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![(at, "@internal".len() as u32)],
        "{:?}",
        report.diagnostics
    );
}

/// The anchor is the whole Annotation node (`annotation_resolver.dart:245`
/// `.at(node)`): spaced arguments are recovered as arguments when the
/// follower is a reserved `class` head, so the node spans `@` through the
/// closing paren — the manual-face row
/// `annotation_space_before_parenthesis/1` measures `@metadata (x, y)` at
/// 1:1|16. Argument-less annotations keep the `@name` span.
#[test]
fn undefined_annotation_spans_the_annotation_node() {
    let source = "@metadata (a, b)\nclass C {}";
    let report = report(source, &[]);
    let at = source.find("@metadata").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_annotation"),
        vec![(at, "@metadata (a, b)".len() as u32)],
        "{:?}",
        report.diagnostics
    );
}

/// The drift corpus face: a bare top-level creation send inside a
/// broken-chain class. The undecided member question falls through to the
/// scoped lookup, which answers with the imported class — the send is the
/// class's creation, never an implicit-`this` member, so no
/// `undefined_method` fires (the corpus's 17 false rows; the landed
/// `Found`-only claim face).
#[test]
fn bare_creation_send_in_broken_chain_class_resolves() {
    let dep = "class ArgumentsForBatchedStatement {\n\
              \x20 const ArgumentsForBatchedStatement();\n\
              }";
    let source = "class Host extends Base {\n\
                  \x20 void m() {\n\
                  \x20   ArgumentsForBatchedStatement();\n\
                  \x20 }\n\
                  }";
    let report = report(
        source,
        &[
            ("dep0.dart", dep, true),
            ("dep1.dart", "class Base {}", false),
        ],
    );
    assert_eq!(
        codes(&report, "undefined_method"),
        vec![],
        "{:?}",
        report.diagnostics
    );
}

/// The mirror guard: a real member call over a receiver whose chain breaks
/// at a frozen SDK face still reports `undefined_method` — the fall-through
/// must not bleed the scope answer into the member face. (`Match` is the
/// frozen interface the verdict answers outright; a break at a genuinely
/// unindexed declaration keeps the designed under-report instead.)
#[test]
fn member_call_over_broken_chain_receiver_reports() {
    let source = "class C extends Match {\n\
                  \x20 void f(C c) {\n\
                  \x20   c.zorch();\n\
                  \x20 }\n\
                  }";
    let report = report(source, &[]);
    let zorch_at = source.find("zorch").unwrap() as u32;
    assert_eq!(
        codes(&report, "undefined_method"),
        vec![(zorch_at, 5)],
        "{:?}",
        report.diagnostics
    );
}
