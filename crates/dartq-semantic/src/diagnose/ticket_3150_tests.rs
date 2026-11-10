//! The #3150 pins: `undefined_identifier`'s InvalidType operand face —
//! the numeric refinement reads a resolution failure the oracle also
//! sees as `InvalidType`, so `int - <unresolved>` answers `double` and
//! the `int` sink reports `invalid_assignment` beside the miss itself.
//!
//! Upstream: the oracle's `_refineNumericInvocationTypeNullSafe`
//! (pkg/analyzer/lib/src/dart/element/type_system.dart:2058-2121) types
//! `e1 - e2` on a `num`-family receiver from the operand types, and its
//! `S <: double?` test reads `InvalidType` as a subtype of everything
//! (subtype.dart `_isSubtypeOf`'s `T0 is InvalidTypeImpl || T1 is
//! InvalidTypeImpl` arm). The receiver-side rule — a member read on a
//! receiver whose written head resolves to nothing types the read
//! `InvalidType`, and an unresolved prefixed member read
//! (`undefined_prefixed_name`) types `InvalidType` — is
//! type_property_resolver.dart's error-element path: the lookup never
//! runs on an `InvalidType` receiver, and an error element's read face
//! is the invalid type. A declared receiver whose member the model
//! cannot spell stays the documented gap (an extension the pass-2
//! applicability walk has not run yet may still supply the member).
//!
//! Oracle rows: `dart analyze` 3.13.7 over the /tmp/ui3150 battery —
//! `int i = 1 - Missing.x;` reports `invalid_assignment` +
//! `undefined_identifier`, `int i = 1 - p.notThere;` reports
//! `invalid_assignment` + `undefined_prefixed_name`, and
//! `int i = 1 - a.b;` on `a: int` reports `invalid_assignment` +
//! `undefined_getter` (the last stays `Gap` here — the member face's
//! extension applicability is not this face's question).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The member-bearing prelude: `int`'s `-`/`+` operators typed `num →
/// num` so the refinement's `S <: double?` question is real, and the
/// names-only `dart:core` stub resolves the special spellings.
const CORE_PRELUDE: &str = "class num {} \
     class int extends num { int operator +(int o) => this; int operator -(int o) => this; } \
     class double extends num {} class String {} class bool {} \
     class Iterable<E> {} class List<E> extends Iterable<E> {} \
     class Map<K, V> {} class Set<E> extends Iterable<E> {} class Exception {} \
     abstract class Enum { int get index => 0; }\n";

const CORE_STUB_NAMES: [&str; 10] = [
    "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type", "Symbol",
];

/// Diagnoses `source` beside [`CORE_PRELUDE`] as the sole file of a
/// library importing the names-only `dart:core` stub.
fn report(source: &str) -> FileReport {
    let full = format!("{CORE_PRELUDE}{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(&full, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    for name in CORE_STUB_NAMES {
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
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
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
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// Diagnoses `source` beside [`CORE_PRELUDE`] importing a sibling file
/// under `prefix` — the `Receiver::Prefix` arm's real prefix edge.
fn report_prefixed(source: &str, source_a: &str, prefix: &str) -> FileReport {
    let full = format!("{CORE_PRELUDE}{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned = dartq_scanner::scan(
        &full,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements = collect(&full, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    for name in CORE_STUB_NAMES {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    for (name, decl) in elements_a.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: Some(strings.intern(prefix)),
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a);
    index.insert(file_m, elements.clone());
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        &full,
        &scanned.tokens,
        features,
        &strings,
        file_m,
        lib_m,
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

/// A bare-head member read on a resolution miss (`x.y` with `x`
/// undefined) is the oracle's `InvalidType` operand: `1 - x.y` refines
/// to `double`, and the `int` sink reports `invalid_assignment` beside
/// the receiver's own `undefined_identifier` (subtype.dart's
/// `InvalidTypeImpl` subtype-of-everything arm).
#[test]
fn missing_receiver_head_operand_reports_invalid_assignment() {
    for head in ["x", "Missing", "p"] {
        let src = format!("void f() {{\n  int i = 1 - {head}.y;\n}}");
        let report = report(&src);
        let names = codes(&report);
        assert!(
            names.contains(&"invalid_assignment"),
            "{head}: {names:?} — an InvalidType operand refines `1 - _` to double"
        );
        assert!(
            names.contains(&"undefined_identifier"),
            "{head}: {names:?} — the receiver head reports its own miss"
        );
    }
}

/// A resolved local's member read (`f.y` with `f` a declared value)
/// keeps the model's gap — the receiver head is no resolution failure,
/// so no `invalid_assignment` fires even when the member itself is
/// model-invisible.
#[test]
fn resolved_receiver_head_operand_stays_a_gap() {
    let report = report("void f() {\n  var u = 0;\n  int i = 1 - u.y;\n}");
    let names = codes(&report);
    assert!(
        !names.contains(&"invalid_assignment"),
        "{names:?} — a declared receiver is no InvalidType"
    );
}

/// A prefixed member read through a real import prefix resolves in the
/// prefix's export scope alone: a miss reports `undefined_prefixed_name`
/// and the read types `InvalidType` — the `-` refinement answers
/// `double` and the `int` sink reports.
#[test]
fn missing_prefixed_member_operand_reports_invalid_assignment() {
    let report = report_prefixed(
        "import 'a.dart' as p;\nvoid f() {\n  int i = 1 - p.notThere;\n}",
        "class A {}\n",
        "p",
    );
    let names = codes(&report);
    assert!(
        names.contains(&"invalid_assignment"),
        "{names:?} — an unresolved prefixed read is InvalidType"
    );
    assert!(
        names.contains(&"undefined_prefixed_name"),
        "{names:?} — the prefixed miss reports its own code"
    );
}

/// A prefixed member read that resolves (`p.A` exports `A`)
/// is the declared element's face — no `InvalidType`, no
/// `invalid_assignment`, and no `undefined_prefixed_name`.
#[test]
fn resolved_prefixed_member_operand_stays_silent() {
    let report = report_prefixed(
        "import 'a.dart' as p;\nvoid f() {\n  int i = 1 - p.A;\n}",
        "class A {}\n",
        "p",
    );
    let names = codes(&report);
    assert!(
        !names.contains(&"invalid_assignment"),
        "{names:?} — a resolved prefixed member is no InvalidType"
    );
    assert!(!names.contains(&"undefined_prefixed_name"), "{names:?}");
}

// —— the code-split table (oracle 3.13.7): bare expression miss →
// `undefined_identifier`; type-position miss → `undefined_class`;
// invocation miss → `undefined_function`/`undefined_method` on the
// receiver's host. ————————————————————————————————————————————————

/// `x;` where `x` is undeclared reports `undefined_identifier` at the
/// name — the bare-read miss (error_verifier.dart's
/// `_checkForUndefinedIdentifier`).
#[test]
fn bare_read_miss_reports_undefined_identifier() {
    let report = report("void f() {\n  x;\n}");
    let names = codes(&report);
    assert_eq!(names, vec!["undefined_identifier"], "{names:?}");
}

/// `p.C c;` with `p` undeclared reports `undefined_class` at `C` — the
/// type-position miss (named_type_resolver.dart's `reportNullOrNonType
/// Element` importPrefix fallthrough), never `undefined_identifier`.
#[test]
fn prefixed_type_head_miss_reports_undefined_class() {
    let report = report("void f() {\n  p.C c = throw 0;\n  c;\n}");
    let names = codes(&report);
    assert!(names.contains(&"undefined_class"), "{names:?}");
    assert!(
        !names.contains(&"undefined_identifier"),
        "{names:?} — the type-position miss codes undefined_class"
    );
}

/// `p.C.m();` with `p` undeclared reports `undefined_identifier` at `p`
/// — the expression-position head miss; the continuation's member faces
/// never run on an InvalidType receiver.
#[test]
fn prefixed_expression_head_miss_reports_undefined_identifier() {
    let report = report("void f() {\n  p.C.m();\n}");
    let names = codes(&report);
    assert_eq!(names, vec!["undefined_identifier"], "{names:?}");
}

/// A bare write miss (`x = 1`) reports `undefined_identifier` — the
/// write arm of the same guarded emission.
#[test]
fn bare_write_miss_reports_undefined_identifier() {
    let report = report("void f() {\n  x = 1;\n}");
    let names = codes(&report);
    assert_eq!(names, vec!["undefined_identifier"], "{names:?}");
}

/// A bare invocation miss outside any container (`f()` with `f`
/// undeclared) reports `undefined_function`, never
/// `undefined_identifier` — the `UnqualifiedLookupDomain`'s no-this arm
/// (method_invocation_resolver.dart:424-431).
#[test]
fn bare_call_miss_reports_undefined_function() {
    let report = report("void f() {\n  g();\n}");
    let names = codes(&report);
    assert_eq!(names, vec!["undefined_function"], "{names:?}");
}

/// A declared class's missing static member (`C.m()` with `C` declared,
/// `m` absent) reports `undefined_method` — the resolved receiver's
/// member face, never `undefined_identifier`.
#[test]
fn declared_class_member_miss_reports_undefined_method() {
    let report = report("class C {}\nvoid f() {\n  C.m();\n}");
    let names = codes(&report);
    assert_eq!(names, vec!["undefined_method"], "{names:?}");
}

/// A `Bare(Missing)` operand on the `on`-constraint face of a
/// mixin/extension body stays the documented gap — an opaque `on` may
/// declare the name through constraints the model cannot walk (probed
/// `mixin M on C { int i = 1 - x; }` reports `invalid_assignment`
/// beside `undefined_identifier` because `C` is resolvable).
#[test]
fn mixin_opaque_on_bare_miss_stays_a_gap() {
    let report = report("class C {}\nmixin M on C {\n  m() {\n    int i = 1 - x;\n  }\n}");
    let names = codes(&report);
    assert!(names.contains(&"undefined_identifier"), "{names:?}");
    assert!(
        names.contains(&"invalid_assignment"),
        "{names:?} — a resolvable `on` face keeps the InvalidType operand"
    );
}
