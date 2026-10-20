//! The #2422 pins (the #2415 recon fix wave, the D4/D5/D6 slice this lane
//! owns; D2/D3's seams sit in sibling-owned files and carry precise
//! re-dispatch notes instead of pins here).
//!
//! D5: an explicit-keyword unnamed creation (`const C(...)`, `new C(...)`)
//! stacks its value row like the named-selector fold — a following member
//! merge resolves against the class (`const C().convert(text)` is silent),
//! never against a stale row an earlier body left (`UNDEFINED_METHOD` for
//! `convert` of `String` was the divergence).
//!
//! D6: an unqualified invocation whose head carries a type-argument group
//! (`Undefined<X>(...)`) links the head as the send's callee even when the
//! group's argument is unresolved — the oracle's invocation rewrite reports
//! `undefined_method` of the enclosing type in an instance body and
//! `undefined_function` at top level, never the bare-read
//! `undefined_identifier`.
//!
//! D4: a written type whose head resolves to no type is `InvalidType` —
//! assignable to everything — so the assignment, declaration-initializer,
//! and return faces ask no question of it (`expression = parent` over an
//! `AstNode?` parameter stays silent; a real-typed control still reports).
//!
//! Oracle provenance: probed against `dart analyze` (Dart 3.13.1, fvm
//! 3.47.1) on the /tmp/probe2422 fixtures.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

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
            "Object", "String", "int", "bool", "double", "num", "List", "Map", "void",
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

fn has(names: &[&'static str], code: &str) -> bool {
    names.contains(&code)
}

const CONVERTER: &str = "class C {\n\
                         \x20 const C();\n\
                         \x20 C.named();\n\
                         \x20 String convert(String s) => s;\n\
                         }\n\
                         void g(String s) {}\n";

/// D5: the const unnamed creation's member resolves against the class — the
/// value row the fold stacks answers `.convert`, so the send is silent (the
/// divergence reported `undefined_method` of `String`, the stale row an
/// earlier body left).
#[test]
fn const_creation_member_resolves_against_the_class() {
    let source = "void f(String text) {\n\
                  \x20 g(const C().convert(text));\n\
                  }\n";
    let names = code_names(&report(&format!("{CONVERTER}{source}")));
    assert!(!has(&names, "undefined_method"), "{names:?}");
    assert!(!has(&names, "undefined_getter"), "{names:?}");
}

/// D5: the `new` spelling shares the fold.
#[test]
fn new_creation_member_resolves_against_the_class() {
    let source = "void f(String text) {\n\
                  \x20 g(new C().convert(text));\n\
                  }\n";
    let names = code_names(&report(&format!("{CONVERTER}{source}")));
    assert!(!has(&names, "undefined_method"), "{names:?}");
}

/// D5: the named-selector fold keeps its posture (regression guard).
#[test]
fn named_creation_member_still_resolves() {
    let source = "void f(String text) {\n\
                  \x20 g(C.named().convert(text));\n\
                  }\n";
    let names = code_names(&report(&format!("{CONVERTER}{source}")));
    assert!(!has(&names, "undefined_method"), "{names:?}");
}

/// D6: the type-argumented unqualified invocation in an instance body codes
/// `undefined_method` of the enclosing type (the oracle's invocation
/// rewrite), never `undefined_identifier`; the undefined type argument
/// keeps its own face.
#[test]
fn type_argumented_invocation_codes_the_method_face() {
    let source = "class Host {\n\
                  \x20 void f() {\n\
                  \x20   Undefined2Impl<Missing2>(resolver: this, node: null);\n\
                  \x20 }\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_method"), "{names:?}");
    assert!(has(&names, "non_type_as_type_argument"), "{names:?}");
    assert!(!has(&names, "undefined_identifier"), "{names:?}");
}

/// D6: at top level the same shape codes `undefined_function`.
#[test]
fn type_argumented_invocation_codes_the_function_face() {
    let source = "void f() {\n\
                  \x20 Undefined2Impl<Missing2>(3);\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_function"), "{names:?}");
    assert!(has(&names, "non_type_as_type_argument"), "{names:?}");
    assert!(!has(&names, "undefined_identifier"), "{names:?}");
}

/// D4: the invalid-head right side (`AstNode?` over an undefined class) is
/// assignable to everything — the assignment stays silent beside the
/// undefined-class row.
#[test]
fn invalid_head_rhs_is_assignable() {
    let source = "class Expression {}\n\
                  void f(Expression expression, AstNode? parent) {\n\
                  \x20 expression = parent;\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_class"), "{names:?}");
    assert!(!has(&names, "invalid_assignment"), "{names:?}");
}

/// D4: the return face shares the gate — `return parent;` against an `int`
/// host reports the undefined class alone.
#[test]
fn invalid_head_return_stays_silent() {
    let source = "int g(AstNode? parent) {\n\
                  \x20 return parent;\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_class"), "{names:?}");
    assert!(!has(&names, "return_of_invalid_type"), "{names:?}");
}

/// D4: the declaration-initializer face shares the gate.
#[test]
fn invalid_head_decl_initializer_stays_silent() {
    let source = "void g(AstNode? parent) {\n\
                  \x20 int x = parent;\n\
                  \x20 x;\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "undefined_class"), "{names:?}");
    assert!(!has(&names, "invalid_assignment"), "{names:?}");
}

/// D4 control: a real-typed nullable source still reports — the gate
/// over-silences nothing.
#[test]
fn real_type_control_still_reports_invalid_assignment() {
    let source = "class Expression {}\n\
                  class Real {}\n\
                  void f(Expression expression, Real? parent) {\n\
                  \x20 expression = parent;\n\
                  }\n";
    let names = code_names(&report(source));
    assert!(has(&names, "invalid_assignment"), "{names:?}");
}
