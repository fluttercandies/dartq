//! The #2324 pins: the dead-coalesce operand faces (`_checkForDeadNullCoalesce`'s
//! left-operand and `??=`-target verdicts) — a `?.`-merged read or send keeps
//! the null union at the right operand (`region?.offset ?? 0` stays silent,
//! the archive corpus's xz_decoder/zip_file rows), an invoked operand reads
//! its send's declared result (`c.m() ?? y` reports when the return is
//! non-nullable), and a generic key send never anchors the dead face. The
//! #2282 companion: an extension body's bare read of an `on`-type field
//! spares `unused_field` on the extended container (the revived
//! `extension_this_read_spares_the_on_type_field` pin carries the named
//! shape; the unnamed-extension spelling rides the same face).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library — the same
/// minimal rig `ticket_2316_tests` carries.
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

fn has_code(report: &FileReport, code: &str) -> bool {
    report
        .diagnostics
        .iter()
        .any(|d| d.code.name == code)
}

/// `region?.offset ?? 0` (the archive corpus's xz_decoder.dart shape): the
/// `?.`-merged read keeps the null union, so the right operand is reachable
/// and no dead row reports even though the field itself is non-nullable.
#[test]
fn null_aware_read_lhs_keeps_the_dead_rhs_alive() {
    let source = "class int {} \
                  class Region { int offset = 0; } \
                  int f(Region? region) => region?.offset ?? 0;";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `c?.m() ?? y`: a `?.`-merged send's result is nullable regardless of the
/// method's declared return — the right operand stays reachable.
#[test]
fn null_aware_invoke_lhs_keeps_the_dead_rhs_alive() {
    let source = "class int {} \
                  class C { int m() => 0; } \
                  int f(C? c, int y) => c?.m() ?? y;";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `c.m() ?? y` over a non-nullable declared return: the send's static type
/// is the return type, the left operand can't be null, and the right
/// operand reports the dead face (the oracle's `_checkForDeadNullCoalesce`
/// invoke arm).
#[test]
fn invoked_non_null_lhs_reports_the_dead_rhs() {
    let source = "class int {} \
                  class C { int m() => 0; } \
                  int f(C c, int y) => c.m() ?? y;";
    let rep = report(source);
    assert!(
        has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `c.m() ?? y` over a nullable declared return: the send's result can be
/// null — the right operand stays reachable.
#[test]
fn invoked_nullable_lhs_stays_silent() {
    let source = "class int {} \
                  class C { int? m() => 0; } \
                  int f(C c, int y) => c.m() ?? y;";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// `m[key(v)] ?? d`: the invoked key's nullable return never anchors the
/// dead face — the index read's own type (`Map.[]`'s `V?`) governs, and the
/// key send row is not the coalesce operand (#2172's `??`-face posture).
#[test]
fn invoked_key_never_anchors_the_dead_coalesce_rhs() {
    let source = "class Object {} class V {} \
                  class M { V? operator [](Object? k) => null; } \
                  Object? key(Object? o) => o; \
                  void f(M m, V v, V d) { var x = m[key(v)] ?? d; }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "dead_null_aware_expression"),
        "{:?}",
        rep.diagnostics
    );
}

/// An extension body's implicit-`this` read of an `on`-type field spares
/// the field: the unnamed-extension spelling rides the same owner face as
/// the named one (#2282's revived pin covers `extension X on C`).
#[test]
fn unnamed_extension_this_read_spares_the_on_type_field() {
    let source = "class C { \
                    int _n = 0; \
                  } \
                  extension on C { \
                    int get doubled => _n * 2; \
                  } \
                  void f(C c) { print(c.doubled); }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "unused_field"),
        "{:?}",
        rep.diagnostics
    );
}

/// An extension body's bare read never spares the extension's OWN
/// same-spelled field through the `on` face: the owner is whichever
/// container the resolution answered, and a field only the extension
/// declares is the extension's own (the `on`-type climb must not leak the
/// credit to an unrelated container's field).
#[test]
fn extension_own_field_read_still_marks_the_extension() {
    let source = "class C { \
                    int _n = 0; \
                  } \
                  class D { \
                    int _n = 1; \
                  } \
                  extension X on C { \
                    int get doubled => _n * 2; \
                  } \
                  void f(C c, D d) { print(c.doubled); print(d._n); }";
    let rep = report(source);
    // C._n is read through the extension (spared); D._n is read directly
    // (spared). The pin: neither spelling leaks a false negative —
    // specifically C._n's credit must land on C, not D.
    assert!(
        !has_code(&rep, "unused_field"),
        "{:?}",
        rep.diagnostics
    );
}
