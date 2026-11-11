//! #3828 pins — the zero-span (inferred) type-argument gate in the
//! invocation-site bounds face.
//!
//! [`Engine::publish_creation_instantiation`] publishes a solved
//! creation's arguments with zero-length spans, its contract promising the
//! bounds faces stay inert — but
//! [`Engine::check_call_type_arg_bounds`] iterated `0..len` and only read
//! `spans[index]` as the report anchor, so the inferred arguments rode
//! into the verdict. Upstream bounds-checks only a WRITTEN group:
//! `_checkInvocationTypeArguments` returns on `typeArgumentList == null`
//! (upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! type_arguments_verifier.dart:509-511), the invocation inferrer's own
//! check sits inside the `typeArgumentList != null` branch
//! (invocation_inferrer.dart:236-261), and the extension override's
//! `_checkTypeArgumentsMatchingBounds` answers the same way
//! (extension_member_resolver.dart:320).
//!
//! Oracle 3.13.7, measured direction by direction over the inferred
//! shapes (context type, argument-driven, generic-function propagation,
//! parameter/return/list-element context, const, factory, named-selector,
//! tear-off, cascade, enum constant, chained member send — 30+ fixtures):
//! every oracle `type_argument_not_matching_bounds` row anchors a WRITTEN
//! node (the declaration's annotation, the written `<...>` group); the
//! inferred invocation itself is always silent. The gate therefore skips
//! exactly the zero-length spans, and the written faces (declaration row,
//! written-group row) survive untouched.
//!
//! The unit pins below drive the face directly over a hand-built engine:
//! the zero-span entry is inert (fails before the gate — the row fired),
//! and the same entry carrying a real span still reports (the gate keys
//! on the span, never on the entry's origin).

use super::test_rig::{CORE_PRELUDE, code_names, report_ops};
use crate::collector::collect;
use crate::diagnose::{Engine, StrictModes};
use crate::library::ElementIndex;
use crate::model::{MicroKind, Ty};
use dartq_diagnostics::Diagnostic;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    ClassModifiers, Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, SourceRange, StringTable};
use std::path::Path;

/// The bounds face's rows a hand-mounted probe produced: the engine is
/// built over `source` exactly as the rig builds it (scan, collect, the
/// names-only `dart:core` stub library, the declaration graph), `mount`
/// runs the face with whatever [`CallTypeArgs`] the pin constructs, and
/// the `type_argument_not_matching_bounds` rows come back.
fn probe_bounds_face(
    source: &str,
    mount: impl FnOnce(&mut Engine<'_>, &StringTable),
) -> Vec<Diagnostic> {
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
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in [
        "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type",
        "Symbol",
    ] {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
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
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let posture = LibraryClassModifiers::default();
    let mut engine = Engine::new(
        &graph,
        &namespaces,
        &index,
        &strings,
        source,
        &scanned.tokens,
        file,
        lib,
        features.primary_constructors_enabled,
        features.augmentations_enabled,
        features.variance_enabled,
        false,
        features.patterns_enabled,
        StrictModes::default(),
        &posture,
        None,
    );
    mount(&mut engine, &strings);
    engine
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .cloned()
        .collect()
}

/// A `String` argument against a `T extends num` bound — the violation
/// the face decides — mounted with the span the pin chooses.
fn probe_violating_entry(span: SourceRange) -> Vec<Diagnostic> {
    probe_bounds_face("class num {} class String {}", |engine, strings| {
        let string = Ty::named(strings.intern("String"));
        let num = Ty::named(strings.intern("num"));
        let t = strings.intern("T");
        let args = super::CallTypeArgs {
            tys: vec![string].into_boxed_slice(),
            spans: vec![span].into_boxed_slice(),
        };
        engine.check_call_type_arg_bounds(&[(t, Some(num))], &args);
    })
}

// —— the gate: the zero-span inferred placeholder is inert ——————————————

/// The inferred-argument placeholder (`publish_creation_instantiation`'s
/// zero-length spans) checks nothing. Before the gate this emitted
/// `'String' doesn't conform to the bound 'num' of the type parameter
/// 'T'.` anchored at a zero-length span — a row the oracle never reports.
#[test]
fn zero_span_inferred_entry_stays_out_of_the_bounds_face() {
    let rows = probe_violating_entry(SourceRange {
        offset: 7,
        length: 0,
    });
    assert!(rows.is_empty(), "{rows:?}");
}

// —— the control: a real written span still reports ——————————————————————

/// The same violation through a WRITTEN span keeps its row — the gate
/// keys on the span's length, never on the entry's origin. The anchor is
/// the argument's own span, the message the oracle's.
#[test]
fn written_span_entry_still_reports() {
    let rows = probe_violating_entry(SourceRange {
        offset: 7,
        length: 6,
    });
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].message,
        "'String' doesn't conform to the bound 'num' of the type parameter 'T'."
    );
    assert_eq!(rows[0].range.offset, 7);
    assert_eq!(rows[0].range.length, 6);
}

// —— the behavioral shape the oracle pins (report level) ——————————————————

/// `A<String> x = A();` — the creation's instantiation is inferred, and
/// the oracle's single bounds row anchors the DECLARATION's `String`
/// (type_arguments_verifier.dart's `checkNamedType` face). The inferred
/// invocation contributes nothing: exactly one row, at the declaration.
#[test]
fn context_inferred_creation_keeps_only_the_declaration_row() {
    let source = "class A<T extends num> {}\nvoid main() {\n  A<String> x = A();\n}\n";
    let report = report_ops(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
    let analyzed = format!("{CORE_PRELUDE}{source}");
    let declaration_string = analyzed.find("String>").unwrap();
    assert_eq!(rows[0].range.offset as usize, declaration_string);
    assert_eq!(rows[0].range.length, 6);
    assert_eq!(
        rows[0].message,
        "'String' doesn't conform to the bound 'num' of the type parameter 'T'."
    );
}

/// `A<String> x = A<String>();` — the written group checks exactly like
/// the declaration (oracle 3.13.7, two rows: the declaration's `String`
/// and the written argument's `String`). The gate must not eat the
/// written face: both rows stand.
#[test]
fn written_group_creation_keeps_both_written_rows() {
    let source = "class A<T extends num> {}\nvoid main() {\n  A<String> x = A<String>();\n}\n";
    let report = report_ops(source);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .collect();
    assert_eq!(rows.len(), 2, "{:?}", code_names(&report));
    let analyzed = format!("{CORE_PRELUDE}{source}");
    let declaration_string = analyzed.find("String>").unwrap();
    let written_string = analyzed.rfind("String").unwrap();
    let mut offsets: Vec<u32> = rows.iter().map(|d| d.range.offset).collect();
    offsets.sort_unstable();
    assert_eq!(
        offsets,
        vec![declaration_string as u32, written_string as u32]
    );
    for row in &rows {
        assert_eq!(
            row.message,
            "'String' doesn't conform to the bound 'num' of the type parameter 'T'."
        );
    }
}
