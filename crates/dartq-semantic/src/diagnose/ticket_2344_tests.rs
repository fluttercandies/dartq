//! The #2344 pins: the member-read gate on the nullable-condition join —
//! the #2338 shadow-poison's condition face. A staged
//! `NullableCondition` check anchored at a member token is a
//! `receiver.member` read, and the local-variable tables (the flow
//! condition rows and the local promotion staging) must not answer it:
//! upstream reads the condition's resolved static type, and a member
//! access carries its own accessor element — a same-named local never
//! feeds its promotion into the member's condition face
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/bool_expression_verifier.dart:44-73).
//! The flow layer's by-name resolution stages a promoted shadow local's
//! row at the selector span, which pre-fix hid the member's
//! `unchecked_use_of_nullable_value` (the `if (c.flag)` false negative,
//! probed against the pre-fix build). The gate routes member offsets to
//! the property face alone; shadow-free members, the bare local face,
//! and the promotable-field promotion keep their verdicts.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module.
fn report(source: &str) -> FileReport {
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
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
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

// The single-file rig carries no dart:core, so every source declares the
// `bool` face in-source (the `ticket_2324` rig's `class int {}` trick):
// the spelled name rides the engine's closed-bool identity, and the
// undefined-class noise never enters these pins' counts.

// —— #2344 face ①: the shadow-poisoned member condition ——————————————————

/// The probe's w shape: `if (c.flag)` where a same-named local `flag`
/// sits promoted non-null by its own `== null` guard. The oracle reports
/// `unchecked_use_of_nullable_value` — the member reads the field's
/// `bool?`, and the local's promotion is the local's own face. Pre-fix
/// the flow row staged at the selector span answered the join and hid
/// the report.
#[test]
fn member_condition_shadow_of_promoted_local_reports() {
    let source = "class bool {} \
                  class C { bool? flag; } \
                  void f(C c) { \
                    bool? flag = null; \
                    if (flag == null) return; \
                    if (c.flag) {} \
                  }";
    let rep = report(source);
    assert_eq!(
        count_code(&rep, "unchecked_use_of_nullable_value"),
        1,
        "{:?}",
        rep.diagnostics
    );
}

/// The shadow-free member condition keeps the existing verdict: a
/// `bool?` field read as the condition reports exactly once — the gate
/// never suppresses the property face's own question.
#[test]
fn member_condition_without_shadow_still_reports() {
    let source = "class bool {} \
                  class C { bool? flag; } \
                  void f(C c) { if (c.flag) {} }";
    let rep = report(source);
    assert_eq!(
        count_code(&rep, "unchecked_use_of_nullable_value"),
        1,
        "{:?}",
        rep.diagnostics
    );
}

// —— #2344 face ②: the local face the gate must preserve —————————————————

/// The shadow local's own condition read keeps the local semantics: the
/// bare local's offset is no member read, so the local tables answer —
/// the `!= null`-promoted non-null `bool` keeps the inner condition
/// silent even with the same-named member in scope.
#[test]
fn promoted_local_condition_keeps_the_local_face() {
    let source = "class bool {} \
                  class C { bool? flag; } \
                  void f(C c) { \
                    bool? flag = null; \
                    if (flag != null) { if (flag) {} } \
                  }";
    let rep = report(source);
    assert_eq!(
        count_code(&rep, "unchecked_use_of_nullable_value"),
        0,
        "{:?}",
        rep.diagnostics
    );
}

/// The promotable-field promotion survives the gate: `c._flag` over a
/// private final field, tested non-null, promotes to non-null `bool` —
/// the property face alone decides the member offset, and the inner
/// condition stays silent (the `ticket_1351` field-promotion pin's
/// shape, re-keyed on the gate's props arm).
#[test]
fn promotable_member_condition_keeps_the_property_promotion() {
    let source = "class bool {} \
                  class C { final bool? _flag; } \
                  void f(C c) { if (c._flag != null) { if (c._flag) {} } }";
    let rep = report(source);
    assert_eq!(
        count_code(&rep, "unchecked_use_of_nullable_value"),
        0,
        "{:?}",
        rep.diagnostics
    );
}
