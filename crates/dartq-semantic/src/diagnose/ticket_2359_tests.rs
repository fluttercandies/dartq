//! #2359 pins: the body-position function-type capture. A written
//! function type inside an executable's block body (`x is void
//! Function(int)`, a local's annotation) now composes into a real
//! canonically-interned [`crate::model::FnShapeId`] instead of the
//! shapeless `FnShapeId::EMPTY` sentinel, so the shape-keyed sentinels
//! (#2355) decide same-signature `is` checks the way the oracle's
//! resolved `FunctionType` elements do (upstream Dart SDK source,
//! pkg/analyzer — the `_checkAllTypeChecks` verdict reads both sides'
//! resolved types).
//!
//! Oracle provenance: every expectation below was probed with
//! `dart analyze` over the same sources during #2355 (the
//! `/tmp/probe2355` probe family — `control`, `unrelated`,
//! `local_identical`, `local_unrelated`, `contravariant`); the pin set
//! freezes those verdicts at the library-report face.
//!
//! The signature region stays out of the capture on purpose: the
//! executable's return annotation parses with the frame open but the
//! body brace unlatched, and its shapeless degradation is the pinned
//! under-report of the assignability context face
//! (`direct_function_typed_return_spelling_stays_silent`).

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

fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
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
    let elements = collect(source, &scanned.tokens, features, &strings);
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
    )
}

fn has(report: &FileReport, name: &str) -> bool {
    report.diagnostics.iter().any(|d| d.code.name == name)
}

fn code_names(report: &FileReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// ① The control shape: a formal whose written annotation is a real
/// signature tested against the SAME signature spelled in the body —
/// the check is constant-true, the oracle reports one
/// `unnecessary_type_check`. Before the body capture the target armed
/// the shapeless `FnShapeId::EMPTY` sentinel and the verdict stayed
/// silent (the #2355 probe `control.dart`).
#[test]
fn formal_real_signature_is_same_signature_in_body_is_constant_true() {
    let report = unit_report(
        "class int {}\n\
         void t(void Function(int) p) {\n  if (p is void Function(int)) {\n    p;\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ② The different-signature control: a real formal signature tested
/// against a DIFFERENT written signature stays informative — no
/// constant verdict either way (`unrelated.dart`: the returns differ,
/// so the check is not provable and the oracle is silent). Two distinct
/// real shapes key to two distinct sentinels, unprovable at the name
/// level by design (#2355).
#[test]
fn formal_real_signature_is_different_signature_stays_informative() {
    let report = unit_report(
        "class int {}\nclass Object {}\n\
         void t(Object Function(int) p) {\n  if (p is int Function(int)) {\n    p;\n  } else {\n    p;\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ③ The local declaration pair: a local annotated with a function
/// signature binds the REAL interned shape, so the same-signature `is`
/// is constant-true (the oracle reports) and the initializer assignment
/// fits (no `invalid_assignment`). Before the capture the declared
/// shape degraded to `FnShapeId::EMPTY` and the initializer mis-fired
/// `invalid_assignment` (#2355 risk 3, `local_identical.dart`).
#[test]
fn local_function_annotation_binds_real_shape_for_check_and_assignment() {
    let report = unit_report(
        "class S {}\nclass A {}\n\
         void t(A Function(S) f) {\n  A Function(S) p = f;\n  if (p is A Function(S)) {\n    p;\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
    assert!(
        !has(&report, "invalid_assignment"),
        "{:?}",
        code_names(&report)
    );
}

/// ③'s twin: the unrelated local pair stays fully clean — the declared
/// and written shapes are distinct real signatures, so neither the
/// check nor the assignment reports (the oracle is silent on
/// `local_unrelated.dart`). Before the capture both sides keyed to the
/// SAME shapeless sentinel and the check answered a FALSE
/// constant-true `unnecessary_type_check` beside the false
/// `invalid_assignment` (#2355 risk 3).
#[test]
fn local_unrelated_function_pair_stays_clean() {
    let report = unit_report(
        "class S {}\nclass A {}\nclass R {}\n\
         void t(A Function(S) f) {\n  A Function(S) p = f;\n  if (p is R Function(S)) {\n    p;\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check")
            && !has(&report, "invalid_assignment"),
        "{:?}",
        code_names(&report)
    );
}

/// ④ The contravariant shape, pinned at its inherent boundary: `p` is
/// `void Function(Object)`, the target `void Function(String)` — the
/// check IS constant-true for the oracle (contravariant formals:
/// `void Function(String) <: void Function(Object)`, subtype.dart
/// :86-171) and it reports there. At this layer the two signatures key
/// to two distinct shape sentinels, and the name-level world carries no
/// function-subtype rule — the verdict stays informatively silent
/// (`contravariant.dart`). Burning through needs a structural function
/// rule in the check face, not the capture: this pin freezes the
/// boundary so a future rule flips it deliberately.
#[test]
fn contravariant_signature_test_stays_informatively_silent() {
    let report = unit_report(
        "class Object {}\nclass String {}\n\
         void t(void Function(Object) p) {\n  if (p is void Function(String)) {\n    p;\n  } else {\n    p;\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// The signature region control: a direct function-typed RETURN
/// annotation keeps its shapeless degradation (the assignability
/// context face's pinned under-report) — the body gate must not leak
/// into the return position, where the frame is open but the body brace
/// is unlatched.
#[test]
fn return_position_annotation_keeps_shapeless_degradation() {
    let report = unit_report(
        "class DB {}\nclass Str {}\n\
         Str Function() h() {\n  return () => DB();\n}\n",
    );
    assert!(
        !has(&report, "return_of_invalid_type_from_closure"),
        "{:?}",
        code_names(&report)
    );
}
