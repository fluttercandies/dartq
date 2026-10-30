//! The #2860 pins: a switch whose scrutinee is a genuinely-unresolved
//! call (`expr<T>()` with the head resolving nowhere) reads the oracle's
//! `InvalidType` — every subtype relation over it is vacuous
//! (subtype.dart's 0c arm), so the first typed pattern covers the chain
//! (`unreachable_switch_case` on the later cases, `dead_code` past the
//! switch) while the exhaustiveness error stays suppressed
//! (`isAlwaysExhaustive` answers false — no `non_exhaustive_switch_*`).
//!
//! The refusal faces are pinned beside: a head the snapshot's invokable
//! member universe knows (a container member's spelling bare at top
//! level) and a modeled callee whose return reads unknown (an unresolved
//! declared return type) keep the undecidable unknown face — no dead
//! rows invented (the over-deading guard). The join stays idempotent:
//! each dead row reports exactly once.

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the production path the staging gate and the
/// flow's vacuous-cover consumers read.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
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
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
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
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— the vacuous cover ——————————————————————————————————————————————————

/// A switch over a nowhere-resolving generic call: the scrutinee is the
/// oracle's `InvalidType`, the single typed pattern covers vacuously and
/// its `return` ends the chain, so the statement past the switch is dead
/// (`dead_code` at the statement — the oracle's 93:7|21 anchor over
/// switch_statement_error_test.dart:89-93). Both names of `expr<T>()`
/// resolve nowhere (the head and the type argument).
#[test]
fn unresolved_call_scrutinee_covers_vacuously() {
    let src = "class C {}\nvoid f() {\n  var k = 0;\n  switch (expr<T>()) {\n    case C _:\n      return;\n  }\n  k = 1;\n}\n";
    let report = report(src);
    assert_eq!(
        rows(&report, "dead_code"),
        vec![(src.find("k = 1;").unwrap() as u32, 6)],
        "{:?}",
        code_names(&report)
    );
}

/// An invokable member spelling bare at top level (`m()` over `class C`'s
/// own member) keeps the unknown face: the snapshot universe's
/// invokable-member bucket refuses the unresolved-call staging and the
/// case may fail — no dead rows invented (the over-deading guard).
#[test]
fn invokable_member_spelling_keeps_unknown_face() {
    let src = "class C {\n  void m() {}\n}\nvoid f() {\n  var k = 0;\n  switch (m()) {\n    case C _:\n      break;\n  }\n  k = 1;\n}\n";
    let report = report(src);
    assert!(
        rows(&report, "dead_code").is_empty(),
        "{:?}",
        code_names(&report)
    );
    assert!(
        rows(&report, "unreachable_switch_case").is_empty(),
        "{:?}",
        code_names(&report)
    );
}

/// A modeled callee whose declared return type resolves nowhere
/// (`Missing ret()` — the top-level face and the frame-local face) keeps
/// the unknown face: the send resolved (a `Plain` callee row), so the
/// unknown return is the model's own gap, never the oracle's
/// `InvalidType` — no dead rows invented.
#[test]
fn modeled_unknown_return_keeps_unknown_face() {
    let top_level = "class C {}\nMissing ret() => throw 0;\nvoid f() {\n  var k = 0;\n  switch (ret()) {\n    case C _:\n      break;\n  }\n  k = 1;\n}\n";
    let top_report = report(top_level);
    assert!(
        rows(&top_report, "dead_code").is_empty(),
        "{:?}",
        code_names(&top_report)
    );
    let local = "class C {}\nvoid f() {\n  Missing loc() => throw 0;\n  var k = 0;\n  switch (loc()) {\n    case C _:\n      break;\n  }\n  k = 1;\n}\n";
    let local_report = report(local);
    assert!(
        rows(&local_report, "dead_code").is_empty(),
        "{:?}",
        code_names(&local_report)
    );
}

// —— the idempotent join ————————————————————————————————————————————————

/// An unresolved call feeding a local whose `is`-test then deads the else
/// arm: exactly ONE dead row (the else block) — the unmatched chain's
/// join is idempotent, no double dead-report emission.
#[test]
fn invalid_scrutinee_join_reports_each_dead_row_once() {
    let src = "class C {}\nvoid f() {\n  var t = expr<T>();\n  if (t is C) {\n    t.hashCode;\n  } else {\n    print(2);\n  }\n}\n";
    let report = report(src);
    let start = src.find("print(2)").unwrap() as u32 - 6;
    assert_eq!(
        rows(&report, "dead_code"),
        vec![(start, 19)],
        "{:?}",
        code_names(&report)
    );
}

// —— the exhaustiveness suppression —————————————————————————————————————

/// `isAlwaysExhaustive` answers false over the Invalid scrutinee: a
/// non-exhaustive-looking switch (one case, no default) emits no
/// `non_exhaustive_switch_statement` — the hasInvalidType suppression
/// face holds while the cover kills the chain.
#[test]
fn invalid_scrutinee_emits_no_non_exhaustive() {
    let src =
        "class C {}\nvoid f() {\n  switch (expr<T>()) {\n    case C _:\n      break;\n  }\n}\n";
    let report = report(src);
    assert!(
        rows(&report, "non_exhaustive_switch_statement").is_empty(),
        "{:?}",
        code_names(&report)
    );
}
