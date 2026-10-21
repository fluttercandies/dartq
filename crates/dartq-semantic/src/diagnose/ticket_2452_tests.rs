//! The #2452 pins: invocation/creation through a type alias whose RHS
//! names an unresolved class behind an import prefix (census F4,
//! /tmp/probe2452_dirs/{p11,pref,iso} on Dart 3.13.1).
//!
//! `typedef Alias = shared.Foreign<Node>;` with `shared.Foreign` missing
//! grounds nothing the alias can build on. The oracle's faces:
//!
//! - `Alias()` — `invocation_of_non_function` ('Alias' isn't a function),
//!   the element-resolver function-invocation face at the head.
//! - `Alias.named()` — `undefined_method` at the selector: the alias's
//!   value face is `Type`, so `named` resolves against `Type` and misses.
//! - `const Alias()` / `const Alias.named()` / `const Gen<int>()` —
//!   `creation_with_non_type` naming the alias, the instance-creation
//!   resolver's non-type face at the class segment.
//! - `typedef L = io.File;` with `io.File` resolvable — the chain grounds
//!   through the prefix's namespace and stays silent (the iso probe).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source_m` importing `source_a` under prefix `shared` — the
/// alias-RHS-behind-a-prefix rig (`ticket_undefined_family_tests` carries
/// the same two-file shape).
fn report_prefixed(source_m: &str, source_a: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let scanned_a = dartq_scanner::scan(
        source_a,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_a,
    );
    let scanned_m = dartq_scanner::scan(
        source_m,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file_m,
    );
    assert!(scanned_a.diagnostics.is_empty());
    assert!(scanned_m.diagnostics.is_empty());
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: Some(strings.intern("shared")),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
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

fn rows(report: &FileReport) -> Vec<(&'static str, u32)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset))
        .collect()
}

/// `Alias()` over an unresolvable RHS: the bare head reports
/// `invocation_of_non_function` at the alias spelling (probe p11 row
/// `11:3`).
#[test]
fn bare_alias_invocation_reports_non_function() {
    let source_m = "\
import 'a.dart' as shared;

class Node {}

typedef Alias = shared.Foreign<Node>;

void h() {
  Alias();
}
";
    let report = report_prefixed(source_m, "class Other {}");
    let names = code_names(&report);
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "invocation_of_non_function")
            .count(),
        1,
        "{names:?}"
    );
    let at = source_m.find("Alias()").unwrap() as u32;
    assert!(
        rows(&report)
            .contains(&("invocation_of_non_function", at)),
        "{:?}",
        rows(&report)
    );
}

/// `Alias.named()` over an unresolvable RHS: the selector resolves
/// against the alias's `Type` face, so `undefined_method` reports at the
/// `named` token — never at the class segment (probe p11 row `12:17`).
#[test]
fn named_alias_invocation_reports_undefined_method_at_selector() {
    let source_m = "\
import 'a.dart' as shared;

class Node {}

typedef Alias = shared.Foreign<Node>;

void h() {
  var x = Alias.named();
}
";
    let report = report_prefixed(source_m, "class Other {}");
    let at = source_m.find("named").unwrap() as u32;
    let spans = rows(&report);
    assert!(spans.contains(&("undefined_method", at)), "{spans:?}");
}

/// `const Alias.named()` takes the creation face at the class segment —
/// `creation_with_non_type` at `Alias` (probe p11 row `17:9`), never the
/// selector face.
#[test]
fn const_named_alias_reports_creation_with_non_type() {
    let source_m = "\
import 'a.dart' as shared;

class Node {}

typedef Alias = shared.Foreign<Node>;

void i() {
  const Alias.named();
}
";
    let report = report_prefixed(source_m, "class Other {}");
    let at = source_m.find("Alias.named").unwrap() as u32;
    let spans = rows(&report);
    assert!(
        spans.contains(&("creation_with_non_type", at)),
        "{spans:?}"
    );
    assert!(
        !spans.iter().any(|(c, _)| *c == "undefined_method"),
        "{spans:?}"
    );
}

/// `const Gen<int>()` over an unresolvable RHS: the generic alias still
/// reports `creation_with_non_type` at the head (probe p11 row `24:9`).
#[test]
fn const_generic_alias_creation_reports_non_type() {
    let source_m = "\
import 'a.dart' as shared;

typedef Gen = shared.Foreign<int>;

void m() {
  const Gen<int>();
}
";
    let report = report_prefixed(source_m, "class Other {}");
    let at = source_m.find("Gen<int>").unwrap() as u32;
    let spans = rows(&report);
    assert!(
        spans.contains(&("creation_with_non_type", at)),
        "{spans:?}"
    );
}

/// A grounded chain stays silent: `typedef L = io.File;` resolves
/// `File` through the `io` prefix's namespace, so `L()` builds on the
/// class and no invocation/creation face fires (the iso probe's
/// direction — only argument-count/abstract-class rows may remain, which
/// this rig does not exercise).
#[test]
fn grounded_prefixed_alias_stays_silent() {
    let source_m = "\
import 'a.dart' as shared;

typedef L = shared.Real;

void f() {
  L();
}
";
    let report = report_prefixed(source_m, "class Real { Real(); }");
    let names = code_names(&report);
    for code in [
        "invocation_of_non_function",
        "creation_with_non_type",
        "undefined_method",
    ] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

/// An alias whose RHS is another alias hops the chain: `B` unresolvable
/// propagates the non-function face through `A` (the chain rule — the
/// walk does not stop at the first alias hop).
#[test]
fn alias_chain_through_unresolvable_rhs_reports() {
    let source_m = "\
import 'a.dart' as shared;

typedef B = shared.Foreign<int>;
typedef A = B;

void h() {
  A();
}
";
    let report = report_prefixed(source_m, "class Other {}");
    let names = code_names(&report);
    assert!(
        names.contains(&"invocation_of_non_function"),
        "{names:?}"
    );
}
