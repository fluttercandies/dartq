//! The shared single-file diagnosis rig for the `diagnose` test modules:
//! `source` is the sole analyzed file of `package:t`, and the resolved-type
//! snapshot is mounted the way the engine mounts it — the clause-tree
//! capture feeds every container's written supertype trees and parameter
//! bounds into `LibraryTypes::build_with_clauses`, and `fold_unit_with_ops`
//! carries the adapter so the strict arms answer. No SDK is loaded: a
//! names-only `dart:core` stub resolves the special spellings (`Object`,
//! `Never`, `Null`, …) and [`CORE_PRELUDE`] declares the member-bearing
//! core classes a fixture may name.

use super::{FileReport, StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::collector::collect;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use crate::resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::ClassModifiers;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The member-bearing core classes a fixture may name: declared ahead of
/// the fixture on one line so the fixture's own line numbers shift by
/// exactly one (the name layer resolves them as unit declarations; the
/// type registry keeps its builtin rows for the ones it already carries).
pub(super) const CORE_PRELUDE: &str = "class num {} \
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; bool operator >(int o) => true; bool operator <(int o) => true; } \
     class double extends num {} \
     class String { String operator +(String other) => this; int get length => 0; } class bool {} class Iterable<E> {} \
     class List<E> extends Iterable<E> { void add(E e) {} E operator [](int i) => throw 0; int get length => 0; } \
     class Map<K, V> { V? operator [](K k) => null; Iterable<K> get keys => throw 0; } \
     class Set<E> extends Iterable<E> {} class Exception {} \
     abstract class Enum { int get index => 0; }\n";

/// The names-only `dart:core` stub: the special spellings the engine
/// decides by name (`Never` is the bottom type, `Object` the root) resolve
/// as types through an unindexed file, exactly the SDK projection's
/// names-only grade.
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// Diagnoses `source` with the resolved-type snapshot mounted and
/// [`CORE_PRELUDE`] ahead of it.
pub(super) fn report_ops(source: &str) -> FileReport {
    report_ops_bare(&format!("{CORE_PRELUDE}{source}"))
}

/// One imported library of a rig fixture: its `package:` URI, the import
/// prefix the main unit binds it under (`None` for a plain import), and
/// its source (prelude-free; it declares what it names).
pub(super) struct RigImport<'a> {
    pub(super) uri: &'a str,
    pub(super) prefix: Option<&'a str>,
    pub(super) source: &'a str,
}

/// Diagnoses `source` with the resolved-type snapshot mounted and no
/// prelude — the fixture declares every member-bearing class it names.
pub(super) fn report_ops_bare(source: &str) -> FileReport {
    report_ops_with(source, &[])
}

/// [`report_ops`] with `imports` mounted as sibling libraries the main
/// unit imports (prefixed or plain); the prelude rides the main unit.
pub(super) fn report_ops_imports(source: &str, imports: &[RigImport<'_>]) -> FileReport {
    report_ops_with(&format!("{CORE_PRELUDE}{source}"), imports)
}

fn scan_fixture(source: &str, file: dartq_source::FileId) -> dartq_scanner::ScanResult {
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
    scanned
}

fn report_ops_with(source: &str, imports: &[RigImport<'_>]) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = scan_fixture(source, file);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        if matches!(
            decl.kind,
            MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
        ) {
            builder.declare_class(lib, name, file, ClassModifiers::default());
        } else {
            builder.declare(lib, name, file);
        }
    }
    let mut clauses = ClauseTrees::empty();
    crate::capture_clause_trees(
        file,
        source,
        &scanned.tokens,
        features,
        &strings,
        &mut clauses,
    );
    for import in imports {
        let dep_lib = builder.intern(import.uri);
        let dep_file = files.intern(Path::new(import.uri));
        let dep_scanned = scan_fixture(import.source, dep_file);
        let dep_elements = collect(import.source, &dep_scanned.tokens, features, &strings);
        index.insert(dep_file, dep_elements.clone());
        builder.unit(dep_lib, dep_file);
        for (name, decl) in dep_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            if matches!(
                decl.kind,
                MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
            ) {
                builder.declare_class(dep_lib, name, dep_file, ClassModifiers::default());
            } else {
                builder.declare(dep_lib, name, dep_file);
            }
        }
        builder.import(
            dep_lib,
            ImportEdge {
                target: Some(core_lib),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: import.prefix.map(|prefix| strings.intern(prefix)),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        crate::capture_clause_trees(
            dep_file,
            import.source,
            &dep_scanned.tokens,
            features,
            &strings,
            &mut clauses,
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let types = LibraryTypes::build_with_clauses(
        &graph,
        &namespaces,
        &index,
        &strings,
        lib,
        &clauses,
    );
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
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
        set.is_enabled(Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// The diagnostic code names of `report` in source order.
pub(super) fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// `code @offset: message` per row, the failure face of [`assert_silent`].
pub(super) fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// Asserts the report carries no diagnostics, naming every row otherwise.
#[track_caller]
pub(super) fn assert_silent(report: &FileReport) {
    assert!(report.diagnostics.is_empty(), "expected no rows, got {:#?}", rows(report));
}
