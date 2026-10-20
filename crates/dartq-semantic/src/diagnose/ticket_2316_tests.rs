//! The #2316 recipe-(a) pins: a bare read of an inherited field inside a
//! subclass body counts as a read of the declaring container's member —
//! `Sub`'s `_f` read spares `unused_field` on `Base._f` (upstream's
//! `unresolvedReadMembers` credits the member scope's climb, not only the
//! enclosing container's own map). The oracle (fvm default, probed on the
//! `riderrepro` tree) reports nothing on `Base._f` when `Sub` reads it,
//! still reports `unused_field` on an unread sibling field, and spares a
//! static field's bare-name read the same way (#2335 — the official read
//! credit precedes its static early-return).

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

/// A static field's bare-name read inside the subclass marks the field
/// read exactly like an instance read — a static member of the inherited
/// container is in bare-name scope, and the official read credit
/// (`_isReadMember`'s readMembers join) precedes the static
/// early-return, which only gates the override face (#2335).
#[test]
fn static_field_bare_read_spares() {
    let rep = report(
        "class Base { static int _s = 0; }\n\
         class Sub extends Base { int m() => _s; }",
    );
    assert!(
        rows_for(&rep, "unused_field").is_empty(),
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

/// The #2335 recipe pins: static-field read credit.
///
/// The oracle (dart analyze, analyzer-residual triage 2026-10-20 R1) is
/// silent whenever a private static field is READ anywhere in the
/// library — bare same-class, qualified cross-class, or compound
/// read-write. A write-only access never spares.
/// #2335 shape 1: a bare read inside the declaring class spares the
/// static (the member scope's climb now admits statics).
#[test]
fn static_bare_read_spares() {
    let rep = report("class C { static const int _k = 1; int f() => _k; }");
    assert!(
        rows_for(&rep, "unused_field").is_empty(),
        "{:?}",
        rep.diagnostics
    );
}

/// #2335 shape 2: a qualified read through the declaring container's own
/// name (`A._k` in another class of the same library) spares the static —
/// the `Receiver::Container` arm credits the owning container directly.
#[test]
fn static_qualified_cross_class_read_spares() {
    let rep = report(
        "class A { static const int _k = 1; }\n\
         class B { int f() => A._k; }",
    );
    assert!(
        rows_for(&rep, "unused_field").is_empty(),
        "{:?}",
        rep.diagnostics
    );
}

/// #2335 shape 3: a compound read-write (`++_n`) is a read before the
/// write — the pass-2 arm credits any non-write intent.
#[test]
fn static_read_write_spares() {
    let rep = report("class C { static int _n = 0; int f() => ++_n; }");
    assert!(
        rows_for(&rep, "unused_field").is_empty(),
        "{:?}",
        rep.diagnostics
    );
}

/// #2335 negative: a write-only access (`_k = 1`) never reads the
/// implicit getter — the static still reports.
#[test]
fn static_write_only_still_reports() {
    let rep = report("class C { static int _k = 0; void f() { _k = 1; } }");
    assert!(
        !rows_for(&rep, "unused_field").is_empty(),
        "{:?}",
        rep.diagnostics
    );
}
