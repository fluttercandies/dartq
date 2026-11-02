//! The #2353 pins: the `Ty::Unknown` three-state faces — the oracle's
//! `UnknownInferredType` receiver face reports the shared
//! `unchecked_use_of_nullable_value` code, while `dynamic` /
//! `InvalidType` / unresolved-name / `?.` receivers stay silent.
//!
//! Upstream's receiver-type lattice has THREE shapes where dartq's `Ty`
//! has one `Unknown`:
//!   - `InvalidType`        — `isInvalidBounded`, the
//!                            `isDynamicBounded || isInvalidBounded`
//!                            early return at
//!                            type_property_resolver.dart:88-94 keeps the
//!                            access SILENT;
//!   - `dynamic`            — `isDynamicBounded`, the same early return
//!                            keeps it silent;
//!   - `UnknownInferredType`— NEITHER bound, and `isPotentiallyNullable`
//!                            (= `!isNonNullable`,
//!                            type_system.dart:1161-1184) answers true
//!                            for it, so the resolver reaches the
//!                            nullable arm (:103-176) and reports
//!                            `uncheckedMethodInvocationOfNullableValue`
//!                            / the property variant.
//!
//! These pins cover the CLOSURE-INFERRED `UnknownInferredType` face
//! (already converged via the `closure_ctx_greatest_closure` → `Object?`
//! recovery, #2339) plus the four exclusion faces that guard against an
//! over-broad report. The NON-closure star-corpus rows (drift
//! `connection_user.dart`/`manager.dart`, 6 oracle-only UNCHECKED) need
//! a `Receiver`-variant three-state split at `receiver_of_bare`/merge
//! production time — the `Receiver::Unknown` fall-through collapses
//! `dynamic`/`InvalidType`/never-inferred indistinguishably, so no
//! predicate there can separate the faces — and a broken-`package:`
//! import fixture the names-only unit rig cannot express.

use super::*;
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The member-bearing core classes a fixture may name (the shared rig's
/// prelude grade): declared ahead of the fixture so every core spelling
/// resolves.
const CORE_PRELUDE: &str = "class num {} \
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; bool operator >(int o) => true; bool operator <(int o) => true; } \
     class double extends num {} \
     class String { String operator +(String other) => this; int get length => 0; } class bool {} class Iterable<E> {} \
     class List<E> extends Iterable<E> { void add(E e) {} E operator [](int i) => throw 0; int get length => 0; } \
     class Map<K, V> { V? operator [](K k) => null; Iterable<K> get keys => throw 0; } \
     class Set<E> extends Iterable<E> {} class Exception {} \
     abstract class Enum { int get index => 0; }\n";

/// The names-only `dart:core` stub: the special spellings the engine
/// decides by name (`Never` is the bottom type, `Object` the root).
const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// The one-library rig (the shared rig's shape): `source` beside
/// [`CORE_PRELUDE`] is the sole analyzed file of `package:t`, with the
/// names-only `dart:core` stub imported for the special spellings.
fn unit_report(source: &str) -> FileReport {
    let full = format!("{CORE_PRELUDE}{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        &full,
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
    let elements = collect(&full, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        dartq_resolution::ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: dartq_resolution::Combinators::unfiltered(),
        },
    );
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
        &full,
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

/// The unchecked rows of `report` as `(offset, length, message)`
/// tuples — the conformance face without the file column. Offsets are
/// translated back past [`CORE_PRELUDE`] so they address the fixture.
fn unchecked(source: &str) -> Vec<(u32, u32, String)> {
    let shift = CORE_PRELUDE.len() as u32;
    let report = unit_report(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| {
            (
                d.range.offset.saturating_sub(shift),
                d.range.length,
                d.message.to_string(),
            )
        })
        .collect()
}

/// The byte offset of `needle`'s first occurrence (the anchor locator —
/// fixtures keep every spelling unique).
fn at(source: &str, needle: &str) -> u32 {
    source.find(needle).expect(needle) as u32
}

/// The code names of every diagnostic `source` produces (the
/// silent-face probes check the whole report, not only the unchecked
/// family).
fn codes(source: &str) -> Vec<&'static str> {
    let report = unit_report(source);
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— the `UnknownInferredType` receiver face ———————————————————————————
//
// A receiver whose type inference never produced a result lands
// `Receiver::Unknown`, and the flow/re-bind/declaration re-derivation
// all answer `None`. The shape the corpus rows share: a closure whose
// formal types are inferred from a `typedef` whose own parameter
// annotation is unresolved, so the formal's member send runs on the
// never-inferred face.

/// The P5 shape: a typedef's parameter annotation is unresolved, so a
/// closure bound to it infers a formal whose member send reports the
/// unchecked code at the selector (the oracle's
/// `uncheckedMethodInvocationOfNullableValue` —
/// `type_property_resolver.dart` reaches the nullable arm because the
/// formal's `UnknownInferredType` is potentially nullable and neither
/// dynamic- nor invalid-bounded).
#[test]
fn unknown_inferred_formal_invoke_reports_unchecked() {
    // `Qex` never resolves anywhere, so the typedef's `e` formal infers
    // `UnknownInferredType` upstream; the closure binds `e` to it and
    // `e.run(sql)` reports at `run`.
    let src = "typedef Writer<T> = T Function(Qex e, String sql);\n\
               void bind(Writer<int> w) {}\n\
               void m() {\n  bind((e, sql) => e.run(sql));\n}";
    let rows = unchecked(src);
    // The `e.run` selector reports the method variant. The exact count
    // is the mechanism's verdict — one row for the one send.
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, at(src, "run(sql)"));
    assert!(
        rows[0]
            .2
            .starts_with("The method 'run' can't be unconditionally invoked"),
        "{}",
        rows[0].2
    );
}

/// A property read on the never-inferred face reports the property
/// variant — the same nullable arm's non-invoke access.
#[test]
fn unknown_inferred_formal_read_reports_property_variant() {
    let src = "typedef R = void Function(Qex e);\n\
               void bind(R r) {}\n\
               void m() {\n  bind((e) {\n    e.state;\n  });\n}";
    let rows = unchecked(src);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, at(src, "state"));
    assert!(
        rows[0]
            .2
            .starts_with("The property 'state' can't be unconditionally accessed"),
        "{}",
        rows[0].2
    );
}

// —— the exclusion faces ——————————————————————————————————————————————

/// A genuinely `dynamic` receiver stays silent: `var x;` types
/// `dynamic` upstream, which is dynamic-bounded and takes the
/// resolver's early return — the never-inferred face must not absorb
/// it.
#[test]
fn dynamic_receiver_stays_silent() {
    let src = "void m() {\n  var x;\n  x.run();\n  x.state;\n}";
    assert!(
        unchecked(src).is_empty(),
        "{:?}",
        unchecked(src)
    );
}

/// An `InvalidType` receiver stays silent: a member send on a receiver
/// whose head type resolves to nothing takes the invalid-bounded early
/// return — `member_read_invalid` gates it out of the never-inferred
/// arm. The unresolved type name's own `undefined_class` row stands
/// beside it.
#[test]
fn invalid_receiver_stays_silent() {
    let src = "void m(Missing t) {\n  t.run();\n}";
    assert!(
        unchecked(src).is_empty(),
        "{:?}",
        unchecked(src)
    );
    assert!(
        codes(src).contains(&"undefined_class"),
        "{:?}",
        codes(src)
    );
}

/// An unresolved name's member send stays silent on the member face —
/// the receiver read already reported `undefined_identifier`, and the
/// `self_named` latch keeps the never-inferred arm from stacking a
/// second row at the selector.
#[test]
fn unresolved_receiver_reports_its_own_row_only() {
    let src = "void m() {\n  gone.run();\n}";
    let rows = unchecked(src);
    assert!(rows.is_empty(), "{rows:?}");
    assert!(
        codes(src).contains(&"undefined_identifier"),
        "{:?}",
        codes(src)
    );
}

/// A `?.`-guarded access stands down: `e?.run` already unwrapped the
/// receiver, so the nullable arm's unconditional-access verdict does
/// not apply.
#[test]
fn null_aware_access_stays_silent() {
    let src = "typedef Writer<T> = T Function(Qex e, String sql);\n\
               void bind(Writer<int> w) {}\n\
               void m() {\n  bind((e, sql) => e?.run(sql));\n}";
    assert!(
        unchecked(src).is_empty(),
        "{:?}",
        unchecked(src)
    );
}
