//! The #2777 pins: the `invalid_use_of_visible_for_template_member`
//! face. A cross-library reference to an `@visibleForTemplate`
//! (package:angular_meta) member emits the oracle row at the
//! member-name token with the declaring library's URI; a same-library
//! use (`_inCurrentLibrary`) and a template-source use
//! (`_inTemplateSource`) stay silent, and the recognition keys on the
//! stand-in's declared `library angular.meta;` name, not its URI.
//!
//! Oracle rows: dart 3.13 `dart analyze` over the conformance case
//! `manual/diagnostic/invalid_use_of_visible_for_template_member`
//! (`expected.tsv` — the file column is the rig's own and not
//! asserted). Official source: best_practices_verifier.dart
//! `_checkForOtherInvalidAccess` (:1824-1948).

use super::*;
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::{ScanConfig, ScanResult};
use dartq_source::{FileId, FileTable, StringTable};
use std::path::Path;

/// The angular-meta stand-in (verbatim from the conformance mocks tree):
/// the declared `library angular.meta;` name is the oracle's recognition
/// key (`_isTopGetter`, element.dart:2014-2028).
const ANGULAR_META: &str = "library angular.meta;\n\n\
     const _VisibleForTemplate visibleForTemplate = const _VisibleForTemplate();\n\n\
     const _VisibleOutsideTemplate visibleOutsideTemplate =\n    \
         const _VisibleOutsideTemplate();\n\n\
     class _VisibleForTemplate {\n  const _VisibleForTemplate();\n}\n\n\
     class _VisibleOutsideTemplate {\n  const _VisibleOutsideTemplate();\n}\n";

/// The declaring library: `Component.render` carries the annotation.
const DECLARING: &str = "import 'package:angular_meta/angular_meta.dart';\n\n\
     class Component {\n  @visibleForTemplate\n  int render() => 0;\n}\n";

/// The /1 trigger shape (the corpus case verbatim): a cross-library use.
const CROSS_LIBRARY_USE: &str = "import 'other.dart';\n\n\
     void f(Component c) {\n  c.render();\n}\n";

/// The /2 negative shape (the corpus case verbatim): the use and the
/// declaration share one library.
const SAME_LIBRARY_USE: &str = "import 'package:angular_meta/angular_meta.dart';\n\n\
     class Component {\n  @visibleForTemplate\n  int render() => 0;\n}\n\n\
     void g(Component c) {\n  c.render();\n}\n";

/// The rows of `report` over `source` rendered as
/// `SEVERITY|code|line:col|len|message` (source order; 1-based UTF-16
/// line:col), the face the expected.tsv row pins minus the file column.
fn rendered(source: &str, report: &FileReport) -> Vec<String> {
    let mut line_starts = vec![0u32];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(index as u32 + 1);
        }
    }
    let line_col = |offset: u32| {
        let line = line_starts
            .binary_search_by(|start| start.cmp(&offset))
            .unwrap_or_else(|at| at - 1);
        (line + 1, offset - line_starts[line] + 1)
    };
    report
        .diagnostics
        .iter()
        .map(|d| {
            let (line, col) = line_col(d.range.offset);
            let severity = match d.severity {
                Severity::Error => "ERROR",
                Severity::Warning => "WARNING",
                Severity::Info => "INFO",
            };
            format!(
                "{severity}|{}|{line}:{col}|{}|{}",
                d.code.name, d.range.length, d.message
            )
        })
        .collect()
}

/// The libraries `main`'s own import directives name: the declaring
/// library (`import 'other.dart';`, the /1 shape) or the angular-meta
/// stand-in (the /2 shape).
#[derive(Clone, Copy)]
enum EdgeTarget {
    Declaring,
    Meta,
}

/// The names-only `dart:core` stub (the shared rig's grade): the core
/// spellings a fixture names resolve as unit declarations.
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "int",
];

/// Diagnoses `main` (as `main_uri`, the sole diagnosed unit) against a
/// declaring library that imports the angular-meta stand-in, whose graph
/// node carries the declared name `meta_lib_name`, plus `main`'s own
/// import edges. The graph mirrors the engine's build: canonical
/// `package:` URIs, one registered unit per file, the declared
/// `library <name>;` name per node.
fn diagnose(
    main: &str,
    main_uri: &str,
    meta_lib_name: &str,
    main_imports: &[EdgeTarget],
) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("lib/test.dart"));
    let declaring_file = files.intern(Path::new("lib/other.dart"));
    let meta_file = files.intern(Path::new("angular_meta.dart"));
    let scan = |source: &str, file: FileId| -> ScanResult {
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "scanner: {source:?}");
        result
    };
    let scanned = scan(main, file);
    let scanned_declaring = scan(DECLARING, declaring_file);
    let scanned_meta = scan(ANGULAR_META, meta_file);
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(main, &scanned.tokens, features, &strings);
    let declaring_elements = collect(DECLARING, &scanned_declaring.tokens, features, &strings);
    let meta_elements = collect(ANGULAR_META, &scanned_meta.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern(main_uri);
    let declaring_lib = builder.intern("package:test/other.dart");
    let meta_lib = builder.intern("package:angular_meta/angular_meta.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.library_name(meta_lib, meta_lib_name);
    let mut index = ElementIndex::default();
    for (unit_lib, unit_file, unit_elements) in [
        (lib, file, &elements),
        (declaring_lib, declaring_file, &declaring_elements),
        (meta_lib, meta_file, &meta_elements),
    ] {
        builder.unit(unit_lib, unit_file);
        for (name, decl) in unit_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            if matches!(
                decl.kind,
                MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
            ) {
                builder.declare_class(unit_lib, name, unit_file, ClassModifiers::default());
            } else {
                builder.declare(unit_lib, name, unit_file);
            }
        }
        index.insert(unit_file, unit_elements.clone());
    }
    let edge = |target: LibraryId| ImportEdge {
        target: Some(target),
        prefix: None,
        deferred: false,
        combinators: Combinators::unfiltered(),
    };
    builder.import(declaring_lib, edge(meta_lib));
    builder.import(lib, edge(core_lib));
    for target in main_imports {
        builder.import(
            lib,
            edge(match target {
                EdgeTarget::Declaring => declaring_lib,
                EdgeTarget::Meta => meta_lib,
            }),
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    diagnose_unit(
        main,
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
        true,
    )
    .0
}

/// The /1 trigger shape emits the exact oracle row: WARNING tier, the
/// member-name token span (`render`, 4:5, length 6), and the DECLARING
/// library's URI in the `{1}` slot (`package:test/other.dart`, not the
/// using file's).
#[test]
fn cross_library_use_emits_the_oracle_row() {
    let report = diagnose(
        CROSS_LIBRARY_USE,
        "package:test/test.dart",
        "angular.meta",
        &[EdgeTarget::Declaring],
    );
    assert_eq!(
        rendered(CROSS_LIBRARY_USE, &report),
        vec![
            "WARNING|invalid_use_of_visible_for_template_member|4:5|6|The member 'render' can only be used within 'package:test/other.dart' or a template library."
                .to_string()
        ]
    );
}

/// The /2 negative shape stays silent: a use inside the declaring
/// library (`_inCurrentLibrary`, best_practices_verifier.dart:1673-1675)
/// is exempt before the annotation consults.
#[test]
fn same_library_use_stays_silent() {
    let report = diagnose(
        SAME_LIBRARY_USE,
        "package:test/test.dart",
        "angular.meta",
        &[EdgeTarget::Meta],
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

/// A use from a template source is exempt (`_inTemplateSource`,
/// :1602/:1615-1619 — the library's path carries `.template`; mirrored
/// over the canonical URI whose tail preserves the file name).
#[test]
fn template_source_use_stays_silent() {
    let report = diagnose(
        CROSS_LIBRARY_USE,
        "package:test/test.template.dart",
        "angular.meta",
        &[EdgeTarget::Declaring],
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

/// The recognition keys on the declaring stand-in's `library` directive
/// name (`angular.meta`), never its URI: a same-shaped library under
/// another declared name is not angular-meta to the oracle
/// (`_isTopGetter`'s `element.library.name` test, element.dart:2026).
#[test]
fn recognition_keys_on_the_declared_library_name() {
    let report = diagnose(
        CROSS_LIBRARY_USE,
        "package:test/test.dart",
        "not_angular",
        &[EdgeTarget::Declaring],
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}
