//! The #2231 pins: the refutable-pattern face for constant patterns in
//! irrefutable pattern contexts, verified end-to-end through the
//! single-file rig.
//!
//! `(o.f) = v` — a pattern assignment / pattern variable declaration
//! whose extraction target is a parenthesized property access — reports
//! `refutable_pattern_in_irrefutable_context` at the constant pattern's
//! own extent (the face the #2157 parser lane deferred). The emission
//! rides the parser's pattern context (`PatternContext::is_refutable`),
//! the same refutability the official resolver reads; matching contexts
//! (switch statement cases, if-case, switch expressions) are refutable
//! and stay silent. Probe-verified spans against the oracle: `final
//! (o.f) = v;` anchors at `o.f` (length 3), `final (const c) = c;` at
//! `const c` (length 7), and enclosing list/record containers add no row
//! of their own (`final [(1), 2] = [1, 2];` reports exactly the two
//! constant elements).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the sibling ticket rig mirrored (each lane owns its copy while the
/// shared rig's lane is in flight).
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

fn refutable_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "refutable_pattern_in_irrefutable_context")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// `final (o.f) = v;` reports at the constant pattern's full extent
/// (`o.f`, 3 units) — never at the parentheses — with the oracle-verbatim
/// message and correction, and the constant-pattern twin keeps its
/// existing row at the same anchor.
#[test]
fn pattern_declaration_reports_at_property_extent() {
    let source = "class C {\n  int f = 0;\n}\n\nvoid f() {\n  final o = C();\n  int v = 1;\n  final (o.f) = v;\n  print(o.f);\n}\n";
    let report = report(source);
    let rows = refutable_rows(&report);
    let at = source.find("o.f) = v").expect("anchor") as u32;
    assert_eq!(rows, vec![(at, 3)], "{rows:?}");
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "refutable_pattern_in_irrefutable_context")
        .expect("row");
    assert_eq!(row.severity, Severity::Error);
    assert_eq!(
        row.message.as_str(),
        "Refutable patterns can't be used in an irrefutable context."
    );
    assert_eq!(
        row.correction.as_deref(),
        Some("Try using an if-case, a 'switch' statement, or a 'switch' expression instead.")
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"constant_pattern_with_non_constant_expression"),
        "{names:?}"
    );
}

/// The plain assignment form `(o.f) = v;` reports the same way — the
/// pattern-assignment context is irrefutable like the declaration form.
#[test]
fn pattern_assignment_reports_at_property_extent() {
    let source = "class C {\n  int f = 0;\n}\n\nvoid f() {\n  final o = C();\n  (o.f) = 1;\n  print(o.f);\n}\n";
    let report = report(source);
    let rows = refutable_rows(&report);
    let at = source.find("o.f) = 1").expect("anchor") as u32;
    assert_eq!(rows, vec![(at, 3)], "{rows:?}");
}

/// A valid constant in the same position (`final (1) = v;`) still
/// reports — refutability is syntactic, not constancy-dependent — and no
/// constant-pattern twin fires (the constant is valid).
#[test]
fn valid_constant_still_refutable_in_declaration() {
    let source = "void f() {\n  final v = 1;\n  final (1) = v;\n  print(v);\n}\n";
    let report = report(source);
    let rows = refutable_rows(&report);
    let at = source.find("(1) = v").expect("anchor") as u32 + 1;
    assert_eq!(rows, vec![(at, 1)], "{rows:?}");
    let names = code_names(&report);
    assert!(
        !names.contains(&"constant_pattern_with_non_constant_expression"),
        "{names:?}"
    );
}

/// A `const`-prefixed constant pattern reports from the keyword through
/// the operand (`const c`, 7 units).
#[test]
fn const_prefixed_pattern_spans_keyword() {
    let source = "const c = 1;\n\nvoid f() {\n  final (const c) = c;\n  print(c);\n}\n";
    let report = report(source);
    let rows = refutable_rows(&report);
    let at = source.find("const c)").expect("anchor") as u32;
    assert_eq!(rows, vec![(at, 7)], "{rows:?}");
}

/// Matching contexts are refutable: a switch statement case and an
/// if-case pattern stay silent on the refutable face.
#[test]
fn matching_contexts_stay_silent() {
    let declaration = "class C {\n  int f = 0;\n}\n\nvoid f(C o) {\n  switch (o) {\n    case (o.f):\n      break;\n  }\n}\n";
    let names = code_names(&report(declaration));
    assert!(
        !names.contains(&"refutable_pattern_in_irrefutable_context"),
        "{names:?}"
    );
    let if_case = "class C {\n  int f = 0;\n}\n\nvoid f(C o) {\n  if (o case (o.f)) {\n    print(o.f);\n  }\n}\n";
    let names = code_names(&report(if_case));
    assert!(
        !names.contains(&"refutable_pattern_in_irrefutable_context"),
        "{names:?}"
    );
}

/// Enclosing refutable containers add no row of their own: the list
/// pattern reports exactly its two constant elements, never the
/// brackets (the oracle's row set for the same shape).
#[test]
fn container_adds_no_row_beyond_constant_elements() {
    let source = "void f() {\n  final [(1), 2] = [1, 2];\n}\n";
    let report = report(source);
    let rows = refutable_rows(&report);
    let line = source.find("final [").expect("anchor") as u32 + 8;
    assert_eq!(rows, vec![(line, 1), (line + 4, 1)], "{rows:?}");
}

/// An irrefutable variable pattern stays silent — `final (x) = v;`
/// declares `x`; only constant patterns are this face.
#[test]
fn variable_pattern_stays_silent() {
    let source = "void f() {\n  final v = 1;\n  final (x) = v;\n  print(x);\n}\n";
    let names = code_names(&report(source));
    assert!(
        !names.contains(&"refutable_pattern_in_irrefutable_context"),
        "{names:?}"
    );
}

/// `var` declares too: `var (1) = 1;` reports like the `final` form.
#[test]
fn var_declaration_reports() {
    let source = "void f() {\n  var (1) = 1;\n}\n";
    let report = report(source);
    let rows = refutable_rows(&report);
    let at = source.find("(1) = 1").expect("anchor") as u32 + 1;
    assert_eq!(rows, vec![(at, 1)], "{rows:?}");
}
