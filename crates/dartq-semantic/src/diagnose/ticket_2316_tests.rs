//! The #2316 recipe-(a) pins: a bare read of an inherited field inside a
//! subclass body counts as a read of the declaring container's member —
//! `Sub`'s `_f` read spares `unused_field` on `Base._f` (upstream's
//! `unresolvedReadMembers` credits the member scope's climb, not only the
//! enclosing container's own map). The oracle (fvm default, probed on the
//! `riderrepro` tree) reports nothing on `Base._f` when `Sub` reads it,
//! still reports `unused_field` on an unread sibling field, and never
//! credits a static field's bare-name read.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library and returns
/// its report — the same minimal rig `ticket_2313_tests` carries.
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
    assert!(scanned.diagnostics.is_empty(), "scanner diagnostics: {source:?}");
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

fn rows_for<'a>(report: &'a FileReport, code: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect()
}

/// The dio `interceptor.dart:25` shape: `Sub` reads the field `Base`
/// declares — the read rides the member scope's supertype climb, so
/// `Base._f` is used and only the truly unread `_g` reports.
#[test]
fn inherited_field_read_spares_the_declaring_field() {
    let rep = report(
        "class Base { int _f = 0; int _g = 0; }\n\
         class Sub extends Base { int m() => _f; }",
    );
    let rows = rows_for(&rep, "unused_field");
    assert_eq!(rows.len(), 1, "{:?}", rep.diagnostics);
}

/// The direct-member face is unchanged: the enclosing container's own
/// field still spares on a bare read.
#[test]
fn own_field_read_still_spares() {
    let rep = report("class C { int _f = 0; int m() => _f; }");
    assert!(
        rows_for(&rep, "unused_field").is_empty(),
        "{:?}",
        rep.diagnostics
    );
}

/// A static field's bare-name read inside the subclass rides no
/// receiver — it must not mark the field read (upstream's
/// `_useIdentifierElement` only credits instance members through `this`).
#[test]
fn static_field_bare_read_does_not_spare() {
    let rep = report(
        "class Base { static int _s = 0; }\n\
         class Sub extends Base { int m() => _s; }",
    );
    assert!(
        !rows_for(&rep, "unused_field").is_empty(),
        "{:?}",
        rep.diagnostics
    );
}

/// A getter's read still spares through the climb — accessor reads mark
/// nothing on the field face (the member lookup's `Field` kind gate is
/// what keeps accessor-only marks off `unused_field`).
#[test]
fn inherited_getter_read_does_not_spare_a_field() {
    let rep = report(
        "class Base { int get g => 0; int _f = 0; }\n\
         class Sub extends Base { int m() => g; }",
    );
    let rows = rows_for(&rep, "unused_field");
    assert_eq!(rows.len(), 1, "{:?}", rep.diagnostics);
}
