//! The #2159 pins: the `unused_field` read-attribution faces the census
//! counted as the reads-side family of #2156 (webview's `weakThis.target?
//! ._field`, drift's part-declared `_results`/`_db`/`less`/`more`, archive's
//! const-list `blOrder`/`lengthCode`). The library-wide `readMembers`
//! close already stands — `semdiag.rs`'s `staged_fields` pool re-decides
//! each row against the merged `consumed.reads` (upstream
//! `unused_local_elements_verifier.dart` `_isReadMember` +
//! `readMembers`/`unresolvedReadMembers` join, library_analyzer.dart:305-
//! 315); these pins lock the read-side shapes against regression.
//!
//! Oracle provenance: each fixture was probed against `dart analyze` on
//! the same sources (the `/tmp/probe2159` family).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of an import-free library — the
/// single-file rig mirrored from the sibling ticket suites.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    builder.seed_implicit_core(
        lib,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map",
        ],
    );
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The webview face: `weakThis.target?._field` is a read of `_field` via
/// the `?.` chain — the oracle resolves the property access to the
/// getter's element (`readMembers` gains the getter), so the field stays
/// used. A write-only twin still reports.
#[test]
fn null_aware_chain_spares_a_read_field() {
    let source = "class Inner {\n\
                  \x20 int _read = 0;\n\
                  \x20 int _written = 0;\n\
                  }\n\
                  class Outer {\n\
                  \x20 final Inner? _in = Inner();\n\
                  \x20 void go() {\n\
                  \x20\x20\x20\x20_in?._read;\n\
                  \x20\x20\x20\x20_in?._written = 1;\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    let unused = names.iter().filter(|c| **c == "unused_field").count();
    assert_eq!(unused, 1, "{names:?}");
}

/// The drift part-face shape at file scope: a private field read by a
/// getter on the same class — `_db.currentSession()`/`_isOpen` inside a
/// getter body — spares through the same `resolve_member_row` path a
/// cross-part read rides.
#[test]
fn private_field_read_in_getter_spares() {
    let source = "class C {\n\
                  \x20 bool _open = false;\n\
                  \x20 bool get isOpen => _open;\n\
                  \x20 void close() { _open = false; }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"unused_field"), "{names:?}");
}

/// The archive const-list face: a `final` field read inside an instance
/// member (`_opts.length` on `c`) resolves to the implicit getter — the
/// `final` marker never changes the read verdict.
#[test]
fn const_initialized_field_read_spares() {
    let source = "class Cfg {\n\
                  \x20 final int _mode;\n\
                  \x20 const Cfg(this._mode);\n\
                  \x20 static const List<Cfg> all = [Cfg(1)];\n\
                  \x20 bool isDefault() => identical(this, all[0]);\n\
                  \x20 int get mode => _mode;\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"unused_field"), "{names:?}");
}

/// The drift `_Op.less` face: an enum constant accessed through the
/// enum's own name resolves the constant's element — the `members` face
/// credits it, and `unused_field` never reports an accessed constant. An
/// untouched sibling constant stays reported.
#[test]
fn enum_constant_member_access_spares() {
    let source = "enum _Op {\n\
                  \x20 less('<'),\n\
                  \x20 more('>'),\n\
                  \x20 unread('x');\n\
                  \x20 final String op;\n\
                  \x20 const _Op(this.op);\n\
                  }\n\
                  void use(_Op o) {\n\
                  \x20 switch (o) {\n\
                  \x20\x20\x20\x20case _Op.less: print('l');\n\
                  \x20\x20\x20\x20case _Op.more: print('m');\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    let unused = names.iter().filter(|c| **c == "unused_field").count();
    assert_eq!(unused, 1, "{names:?}");
}

/// The control: a genuinely unread private field still reports — the
/// sparing pins above depend on the verdict actually firing when no read
/// resolves. (`+=` alone on a field — `visitCompoundAssignment`'s
/// `readCountsAsUse: parent is! ExpressionStatement` — still reports; that
/// arm lives in `row_resolution.rs`'s `resolve_member_row` and rides a
/// separate lane, so it is pinned by the corpus rather than here.)
#[test]
fn write_only_private_field_still_reports() {
    let source = "class C {\n\
                  \x20 int _neverRead = 0;\n\
                  \x20 int _neverTouched = 0;\n\
                  }\n";
    let names = code_names(&report(source));
    assert_eq!(
        names.iter().filter(|c| **c == "unused_field").count(),
        2,
        "{names:?}"
    );
}
