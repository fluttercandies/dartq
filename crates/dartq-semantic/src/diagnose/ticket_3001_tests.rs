//! #3001 pins: the generic-method type-parameter gate of
//! `invalid_override`.
//!
//! Upstream answers the override verdict as ONE subtype question on the
//! member function types (`correct_override.dart:36-39` →
//! `subtype.dart:338-345` `_isFunctionSubtypeOf`), and the generic arm
//! (`type_system.dart:1581-1635` `relateTypeParameters`) fails it outright
//! when the two signatures disagree on the type-parameter COUNT or on any
//! bound pair (`isEqualTo` under the fresh-parameter unification). The
//! engine's name-level face now mirrors both arms: a `void f<T, S>(T)`
//! over `void f<T>(T)` and a `f<T extends int>` over `f<T extends num>`
//! report exactly like the oracle, while equal-count/equal-bound pairs
//! stay silent. Oracle-verified against `dart analyze` 3.13.x: every row
//! below reproduces the live verdict.
//!
//! Rig convention (the established one in this file's pin modules):
//! `class num {}` / `class int extends num {}` / `class Object {}`
//! declarations stand the name world's primitives in for the fixture, so
//! the heads resolve and `decide_subtype` sees the `int <: num` edge.

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

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module. `types` carries the
/// fixture declarations (`class num {}` …) every case needs.
fn report(types: &str, source: &str) -> FileReport {
    let source = format!("{types}\n{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        &source,
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
    let elements = collect(&source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
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
        &source,
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The primitive face: `num`/`int` declared with their real edge.
const CORE: &str = "class num {}\nclass int extends num {}\nclass Object {}";

/// (a) The contravariant-parameter arm stays silent: the declared `num`
/// parameter accepts every `int` the inherited signature accepts, and
/// the `int` result narrows compatibly (oracle-verified: `num f(num)`
/// over `int f(int)` is a valid override — no `invalid_override`).
#[test]
fn compatible_contravariant_parameter_is_silent() {
    let report = report(
        CORE,
        "class B { int f(int x) => x; }\n\
         class D extends B { int f(num x) => 0; }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"invalid_override"),
        "the widened parameter stays silent: {names:?}"
    );
}

/// (b) The covariant-return arm stays silent: `int` is a subtype of the
/// inherited `num` result (oracle-verified).
#[test]
fn compatible_covariant_return_is_silent() {
    let report = report(
        CORE,
        "class B { num g() => throw 0; }\n\
         class D extends B { int g() => 0; }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"invalid_override"),
        "a result narrowing never breaks the override: {names:?}"
    );
}

/// (c) A dropped required named parameter rejects the calls the
/// inherited signature accepts (oracle-verified `invalid_override`).
#[test]
fn dropped_required_named_parameter_reports() {
    let report = report(
        CORE,
        "class B { void f({required int a}) {} }\n\
         class D extends B { void f() {} }\n",
    );
    let names = code_names(&report);
    assert_eq!(
        names,
        ["invalid_override"],
        "the dropped required named parameter is the oracle's row"
    );
}

/// (d) A required-positional count the inherited callers cannot satisfy
/// (oracle-verified `invalid_override`).
#[test]
fn required_positional_arity_mismatch_reports() {
    let report = report(
        CORE,
        "class B { void f(int a) {} }\n\
         class D extends B { void f(int a, int b) {} }\n",
    );
    let names = code_names(&report);
    assert_eq!(names, ["invalid_override"], "the extra required positional");
}

/// (e) The kind-mismatch split stays out of THIS code: a method over an
/// inherited getter reports under the member family's own row, never
/// `invalid_override` — the oracle's verdict
/// (`CONFLICTING_METHOD_AND_FIELD` upstream).
#[test]
fn method_over_getter_kind_mismatch_is_not_this_code() {
    let report = report(
        CORE,
        "class B { int get g => 0; }\n\
         class D extends B { int g() => 0; }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"invalid_override"),
        "the kind conflict reports under its own code: {names:?}"
    );
}

/// (f) A narrowed type-parameter bound breaks the override:
/// `f<T extends int>` over `f<T extends num>` — the fresh-parameter
/// unification compares the bounds `isEqualTo`-style and `int != num`
/// (oracle-verified `invalid_override`).
#[test]
fn narrowed_type_parameter_bound_reports() {
    let report = report(
        CORE,
        "class B { void f<T extends num>(T x) {} }\n\
         class D extends B { void f<T extends int>(T x) {} }\n",
    );
    let names = code_names(&report);
    assert_eq!(names, ["invalid_override"], "the narrowed bound");
}

/// (g) A dropped bound breaks the override: an unbounded parameter is
/// the oracle's `dynamic` bound, which `isEqualTo` refuses against the
/// inherited `num` (oracle-verified `invalid_override`).
#[test]
fn dropped_type_parameter_bound_reports() {
    let report = report(
        CORE,
        "class B { void f<T extends num>(T x) {} }\n\
         class D extends B { void f<T>(T x) {} }\n",
    );
    let names = code_names(&report);
    assert_eq!(names, ["invalid_override"], "the dropped bound");
}

/// (h) A bound written against the unbounded inherited face breaks the
/// override — `dynamic` and `Object` are not `isEqualTo` (oracle-verified
/// `invalid_override`).
#[test]
fn added_type_parameter_bound_reports() {
    let report = report(
        CORE,
        "class B { void f<T extends Object>(T x) {} }\n\
         class D extends B { void f<T>(T x) {} }\n",
    );
    let names = code_names(&report);
    assert_eq!(names, ["invalid_override"], "the added bound");
}

/// (i) A type-parameter count mismatch breaks the override outright —
/// `relateTypeParameters` returns null before any bound or signature
/// face runs (oracle-verified `invalid_override`).
#[test]
fn type_parameter_count_mismatch_reports() {
    let report = report(
        CORE,
        "class B { void f<T>(T x) {} }\n\
         class D extends B { void f<T, S>(T x) {} }\n",
    );
    let names = code_names(&report);
    assert_eq!(names, ["invalid_override"], "the extra type parameter");
}

/// (j) Equal bounds under DIFFERENT parameter spellings stay silent: the
/// unification renames the declared group onto the inherited spellings
/// before the pairwise bound compare (oracle-verified: `f<S extends
/// num>` over `f<T extends num>` is a valid override).
#[test]
fn equal_bounds_under_renamed_parameters_are_silent() {
    let report = report(
        CORE,
        "class B { void f<T extends num>(T x) {} }\n\
         class D extends B { void f<S extends num>(S x) {} }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"invalid_override"),
        "a renamed-but-equal bound set stays silent: {names:?}"
    );
}

/// (k) A bound pair differing only in nullability breaks the override —
/// `num` and `num?` fail the mutual-subtype leg (oracle-verified
/// `invalid_override`).
#[test]
fn bound_nullability_difference_reports() {
    let report = report(
        CORE,
        "class B { void f<T extends num?>(T x) {} }\n\
         class D extends B { void f<T extends num>(T x) {} }\n",
    );
    let names = code_names(&report);
    assert_eq!(
        names,
        ["invalid_override"],
        "the nullability-different bound"
    );
}

/// (l) A bound that only re-spells the SAME type-parameter group keeps
/// the override valid: `f<T extends num, S extends T>` over itself —
/// the fresh-parameter compare is reflexive (oracle-verified: silent).
#[test]
fn self_equal_referencing_bounds_are_silent() {
    let report = report(
        CORE,
        "class B { void f<T extends num, S extends T>(T t, S s) {} }\n\
         class D extends B { void f<T extends num, S extends T>(T t, S s) {} }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"invalid_override"),
        "identical referencing bounds stay silent: {names:?}"
    );
}
