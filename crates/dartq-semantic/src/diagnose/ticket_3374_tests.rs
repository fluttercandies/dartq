//! #3374 pins — the implicit-`dynamic` static type at a read.
//!
//! # The defect
//!
//! An UNTYPED formal's element type is `dynamic` upstream: the resolver
//! overwrites the fragment's `InvalidTypeImpl` default with
//! `_typeProvider.dynamicType` for a formal that carries neither an
//! annotation nor a function-typed suffix (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart:799-814`),
//! and the closure inference only rewrites a formal whose type is still a
//! `DynamicType` (`function_expression_resolver.dart:106`). The flow layer
//! keeps such a formal's lattice slot at `Ty::Unknown` — its
//! suppress-everything face — so the read-type query answered `Unknown`
//! where the oracle answers `dynamic`, and every `dynamic`-receiver
//! consumer stayed silent (`avoid_dynamic_calls` under-reported
//! `void implicitDynamicType(object) { object.foo(); }`).
//!
//! # The wiring
//!
//! The declaration keeps the conservative lattice face and carries the
//! claim beside it (`VarMeta::implicit_dynamic` in the flow layer); the
//! read-type row (`FlowFact::ReadType` — the row [`Analysis::type_of`]
//! answers) publishes `dynamic` for an UNPROMOTED read of such a variable.
//! A promoted read publishes the promoted type, and a genuinely unknown
//! read keeps `Unknown`, so "known `dynamic`" and "we don't know" stay
//! apart.
//!
//! [`Analysis::type_of`]: crate::analysis::Analysis::type_of

use crate::analysis::{Analysis, analyze_unit};
use crate::diagnose::StrictModes;
use crate::library::ElementIndex;
use crate::model::{MicroKind, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::{FileId, FileTable, SourceRange, StringTable};
use std::path::Path;

/// The names-only `dart:core` face the fixtures resolve through: the
/// spellings land on a file the element index never carries — the engine
/// graph's SDK posture.
const CORE_FACE: [&str; 6] = ["List", "Map", "Set", "Iterable", "Type", "String"];

/// Builds the [`Analysis`] of `source` as the sole indexed unit of
/// `package:t`, with [`CORE_FACE`] mounted as an imported, unindexed
/// library. The context tables leak (`'static`) — the tests are one-shot
/// processes.
fn analyze_core(source: &str) -> Analysis<'static> {
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scan_config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let scanned = dartq_scanner::scan(source, scan_config, file);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let tokens: &'static TokenTable = Box::leak(Box::new(scanned.tokens));
    let elements = crate::collector::collect(source, tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    let core_file: FileId = files.intern(Path::new("dart_core.dart"));
    let core_lib = builder.intern("dart:core");
    builder.unit(core_lib, core_file);
    for name in CORE_FACE {
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
    let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
    let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
    let index: &'static ElementIndex = Box::leak(Box::new(index));
    analyze_unit(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
    )
}

/// The [`SourceRange`] of `needle`'s first occurrence (ASCII fixtures: byte
/// offsets are UTF-16 offsets).
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn span_of(source: &str, needle: &str) -> SourceRange {
    let offset: u32 = source
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {source:?}"))
        .try_into()
        .expect("offset fits u32");
    SourceRange {
        offset,
        length: needle.len() as u32,
    }
}

/// The read of an untyped formal answers `dynamic`: the oracle's
/// `implicitDynamicType` face (`resolution_visitor.dart:799-814` assigns
/// `dynamicType` to a formal with no annotation), which is what a
/// dynamic-receiver lint reports on. The declaration's lattice slot stays
/// the suppress face — this pin covers the query row alone.
#[test]
fn t3374_untyped_formal_read_is_dynamic() {
    let source = "void f(object) { object; }";
    let analysis = analyze_core(source);
    assert_eq!(
        analysis.type_of(span_of(source, "object;")),
        Some(Ty::Dynamic)
    );
}

/// The implicit-`dynamic` face never leaks onto a local: `var x = g();` over
/// a callee whose return the layer cannot type is NOT a dynamic declaration,
/// so its read must not widen to `dynamic` — the silent direction the
/// unknown face exists for.
#[test]
fn t3374_local_read_never_widens_to_dynamic() {
    let source = "void f() { var x = g(); x; }";
    let analysis = analyze_core(source);
    let read = analysis.type_of(span_of(source, "x; }"));
    assert_ne!(
        read,
        Some(Ty::Dynamic),
        "a local's read must never publish the implicit-dynamic face"
    );
}

/// The written-annotation control: a `dynamic` parameter IS `dynamic` at its
/// reads (the face the untyped formal now shares).
#[test]
fn t3374_written_dynamic_formal_read_is_dynamic() {
    let source = "void f(dynamic d) { d; }";
    let analysis = analyze_core(source);
    assert_eq!(
        analysis.type_of(span_of(source, "d; }")),
        Some(Ty::Dynamic)
    );
}

/// An `is`-promoted read of an untyped formal publishes the PROMOTED type,
/// not `dynamic`: inside `if (object is String)` the static type is
/// `String`, so a member call there is the string member, never a dynamic
/// call. The implicit-`dynamic` face only fills the unpromoted reads.
#[test]
fn t3374_promoted_read_of_untyped_formal_is_the_tested_type() {
    let source = "void f(object) { if (object is String) { object; } }";
    let analysis = analyze_core(source);
    let string = analysis.strings().intern("String");
    assert_eq!(
        analysis.type_of(span_of(source, "object; }")),
        Some(Ty::named(string))
    );
}

// —— rider (#3387 residual): a getter read's declared type is its return ———

use super::test_rig::report_ops;

/// A getter read stages its DECLARED RETURN, not `Unknown`: `E(c.bad)` over
/// `void get bad` reaches the void-use face exactly like the field twin
/// (`int? bad`) reaches the assignability face, and reports ONE
/// `use_of_void_result` anchored at the whole `c.bad` property access.
#[test]
fn t3374_rider_getter_read_of_void_getter_use_of_void_result() {
    let (rows, fixture) = getter_rows("void get bad {}", "E(c.bad).f();", "c.bad");
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert_eq!(rows[0].0, "use_of_void_result");
    assert_eq!(rows[0].1, fixture, "anchors at the whole `c.bad` access");
    assert_eq!(rows[0].2, "c.bad".len() as u32);
}

/// The non-void twin stays silent: the getter read's declared `int` is
/// assignable to the extension's `on int`, so the argument check passes.
#[test]
fn t3374_rider_non_void_getter_override_silent() {
    let (rows, ..) = getter_rows("int get bad => 0;", "E(c.bad).f();", "c.bad");
    assert!(rows.is_empty(), "rows: {rows:?}");
}

/// A discard-position read (`c.bad;`) reports nothing: the void-use face
/// fires on a CONSUMED void value, never on a bare expression statement.
#[test]
fn t3374_rider_discard_position_void_getter_silent() {
    let (rows, ..) = getter_rows("void get bad {}", "c.bad;", "c.bad");
    assert!(rows.is_empty(), "rows: {rows:?}");
}

/// `(code, fixture-relative offset, length)` of every void-use /
/// extension-override row `report_ops` emits over the fixture — `class C`
/// declaring `getter`, `statement` inside `void g(C c)` — with the
/// fixture-relative offset of `needle` beside it (the rig prefixes the
/// core prelude, so the offsets are re-based before comparing).
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn getter_rows(getter: &str, statement: &str, needle: &str) -> (Vec<(&'static str, u32, u32)>, u32) {
    let source = format!(
        "extension E on int {{ void f() {{}} }}\n\
         class C {{ {getter} }}\n\
         void g(C c) {{\n  {statement}\n}}"
    );
    let prelude = super::test_rig::CORE_PRELUDE.len() as u32;
    let fixture = source.find(needle).expect("needle in fixture") as u32;
    let report = report_ops(&source);
    let rows = report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "use_of_void_result" | "extension_override_argument_not_assignable"
            )
        })
        .map(|d| {
            (
                d.code.name,
                d.range.offset.saturating_sub(prelude),
                d.range.length,
            )
        })
        .collect();
    (rows, fixture)
}
