//! The #2409 pins: the statement-window read discard on the `unused_field`
//! face — a bare identifier directly under a compound-assignment LHS, a
//! prefix, or a postfix whose parent is the ExpressionStatement is no read
//! (the official `_isReadIdentifier`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart:418-440),
//! so `_x += 1;`/`_x++;`/`-_x;` leave the field unused while `??=`, member
//! targets (`this._x += 1;`, `c._x += 1;`), and binary reads keep theirs.
//! The local-variable face rides the same anchor and is pinned both ways.
//!
//! Oracle provenance: every fixture was probed against `dart analyze`
//! (Dart 3.13.1, fvm default) on the same sources — the `/tmp/probe2409`
//! family (a/b/c/d/e/g/h/i/j/k/l/m/n/o).

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

fn unused_field_count(source: &str) -> usize {
    code_names(&report(source))
        .into_iter()
        .filter(|c| *c == "unused_field")
        .count()
}

/// The defect face: `_x += 1;` as a bare expression statement — the
/// compound read is discarded (`_isReadIdentifier`'s assignment arm), the
/// implicit getter joins no read set, and `unused_field` reports.
#[test]
fn bare_compound_assignment_reports_unused_field() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f() {\n\
                  \x20\x20 _x += 1;\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(unused_field_count(source), 1);
}

/// The control: a later genuine read spares the field.
#[test]
fn compound_with_later_read_spares() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f() {\n\
                  \x20\x20 _x += 1;\n\
                  \x20\x20 print(_x);\n\
                  \x20 }\n\
                  \x20 void print(int x) {}\n\
                  }\n";
    assert_eq!(unused_field_count(source), 0);
}

/// The increment face: `_x++;` discards its operand's read the same way
/// (the `_isReadIdentifier` postfix arm).
#[test]
fn increment_statement_reports_unused_field() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f() {\n\
                  \x20\x20 _x++;\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(unused_field_count(source), 1);
}

/// The prefix face: `-_x;` is a useless read (the `_isReadIdentifier`
/// prefix arm — any prefix operator under an expression statement).
#[test]
fn prefix_negation_statement_reports_unused_field() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f() {\n\
                  \x20\x20 -_x;\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(unused_field_count(source), 1);
}

/// The member-target control: `this._x += 1;` — the identifier's parent is
/// the property access, never the discarded operator, so the read counts
/// and the field stays used (probed: the 3.13.1 oracle reports nothing).
#[test]
fn this_member_compound_target_spares() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f() {\n\
                  \x20\x20 this._x += 1;\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(unused_field_count(source), 0);
}

/// The receiver-target control: `c._x += 1;` spares the same way.
#[test]
fn member_compound_target_spares() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f(C c) {\n\
                  \x20\x20 c._x += 1;\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(unused_field_count(source), 0);
}

/// The `??=` control: if-null assignment reads count as use
/// unconditionally (the official `visitIfNullAssignment` never discards).
#[test]
fn if_null_assignment_spares() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f() {\n\
                  \x20\x20 _x ??= 1;\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(unused_field_count(source), 0);
}

/// The binary-read control: `_x + 1;` is a useful read — the discard never
/// reaches past the prefix/postfix/assignment parents.
#[test]
fn binary_read_spares() {
    let source = "class C {\n\
                  \x20 int _x = 0;\n\
                  \x20 void f() {\n\
                  \x20\x20 _x + 1;\n\
                  \x20 }\n\
                  }\n";
    assert_eq!(unused_field_count(source), 0);
}

/// The local form: `y += 1;` discards the same anchor's local read and
/// `unused_local_variable` reports.
#[test]
fn local_compound_statement_reports_unused_local() {
    let source = "void f() {\n\
                  \x20 var y = 0;\n\
                  \x20 y += 1;\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(
        names.contains(&"unused_local_variable"),
        "{names:?}"
    );
}

/// The local control: a later genuine read spares the variable.
#[test]
fn local_compound_with_read_spares() {
    let source = "void f() {\n\
                  \x20 var y = 0;\n\
                  \x20 y += 1;\n\
                  \x20 print(y);\n\
                  }\n\
                  void print(int x) {}\n";
    let names = code_names(&report(source));
    assert!(
        !names.contains(&"unused_local_variable"),
        "{names:?}"
    );
}
