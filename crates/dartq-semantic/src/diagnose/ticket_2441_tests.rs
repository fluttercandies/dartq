//! The #2441 pins: one argument verdict behind every argument face — the
//! staged teardown's standing rejection plus the two operator-fold walks —
//! routed through the same relation [`Engine::argument_verdict`] answers.
//!
//! Three faces share the row (`arg: T` against a formal `T`):
//!
//! 1. A source head spelling an import prefix is the parser's projection
//!    scar, not a type — `ast.AstNode?` arrives with the prefix token armed
//!    as the `Ty` head, so the nullability matrix fabricates `Some(false)`
//!    over `ast?` and the oracle's silent `Holder(node)` becomes a row.
//!    [`Engine::prefix_head_ty`] gates the relation's first step; the
//!    diagnosing-library `as` face covers in-file spellings, the graph-wide
//!    face covers a foreign `Boundary::ty` projection (`link.dart`'s
//!    `ast.AstNode?` return feeding `simply_bounded.dart`, which never
//!    registers `ast` — the 13 census rows' shape).
//! 2. [`Engine::assignable`] decides everything it resolves — including
//!    the declared supertype graph's `implements` climb, where the
//!    interface-narrowed `ClassDeclaration` argument into a bare `AstNode`
//!    formal already proves assignable through the three-hop chain
//!    (`ClassDeclaration → CompilationUnitMember → Declaration →
//!    AnnotatedNode → AstNode`).
//! 3. On the matrix's `None` cut between two bare named heads,
//!    [`NameWorld::decide_subtype`] walks the same declared edges the
//!    switch tracker uses — the consistency leg, never a second lattice.
//!
//! Oracle provenance: /tmp/probe2441 (3.13.1 silent on every pin's
//! positive shape; `corpus.txt` carries the real-corpus `ast?` message
//! the pre-fix pass reported at `simply_bounded.dart:102:11`).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole unit of a no-import library — the
/// single-file rig `ticket_2432_tests` carries.
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

/// Diagnoses `main_source` importing `lib_source` under `prefix` — the
/// foreign-projection rig `ticket_2432_tests`' `report_prefixed` carries,
/// extended to a second bare import of the same target so the diagnosing
/// file also sees the library's own bare names (the corpus shape:
/// `simply_bounded.dart` imports `ast.dart` unprefixed while `link.dart`
/// declares the `ast.AstNode?` return the boundary projects).
fn report_dual(main_source: &str, lib_source: &str, prefix: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_lib = files.intern(Path::new("lib.dart"));
    let scan = |source: &str, file: FileId| {
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
        scanned
    };
    let scanned_main = scan(main_source, file_main);
    let scanned_lib = scan(lib_source, file_lib);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main_source, &scanned_main.tokens, features, &strings);
    let elements_lib = collect(lib_source, &scanned_lib.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_lib = builder.intern("package:t/lib.dart");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_lib, elements_lib.clone());
    for (name, decl) in elements_main.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_main, name, file_main);
    }
    for (name, decl) in elements_lib.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_lib, name, file_lib);
    }
    // The bare import the corpus diagnosing file carries.
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    // The prefixed import the foreign projection's `ast.` head came from.
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_lib),
            prefix: Some(strings.intern(prefix)),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main_source,
        &scanned_main.tokens,
        features,
        &strings,
        file_main,
        lib_main,
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
    report.diagnostics.iter().any(|d| d.code.name == code)
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// The 3-hop `implements` chain: `node` narrowed by `is ClassDeclaration`
/// to the interface's own chain walks `ClassDeclaration →
/// CompilationUnitMember → Declaration → AnnotatedNode → AstNode`, and the
/// `AstNode` formal accepts it silently. The assignability matrix proves
/// this through `implements_interface` — the census premise that the
/// relation routed around it was falsified; the pin guards the fix stays.
#[test]
fn narrowed_implements_chain_argument_is_silent() {
    let source = "class AstNode {} \
                  class AnnotatedNode implements AstNode {} \
                  class Declaration implements AnnotatedNode {} \
                  class CompilationUnitMember implements Declaration {} \
                  class ClassDeclaration implements CompilationUnitMember {} \
                  class Holder { final AstNode node; Holder(this.node); } \
                  void run(AstNode? node) { \
                  if (node is ClassDeclaration) { Holder(node); } \
                  }";
    let rep = report(source);
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// A genuine unrelated mismatch still reports: `String` against a bare
/// `int` formal — the matrix's primitive-chain `Some(false)` keeps its
/// verdict, so the row the oracle emits must not regress.
#[test]
fn genuine_mismatch_still_reports() {
    let source = "void take(int i) {} \
                  void run() { take(\"x\"); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// A nullable source into a non-nullable formal keeps its verdict:
/// `String?` against `String` is the Left-Nullable `Some(false)` the
/// oracle reports — the prefix gate only disarms a projection scar, never
/// a typed pair.
#[test]
fn nullable_into_nonnullable_keeps_the_rejection() {
    let source = "void take(String s) {} \
                  void run(String? s) { take(s); }";
    let rep = report(source);
    assert!(
        has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}

/// The corpus-derived shape: a foreign function returning `ast.AstNode?`
/// (the prefix token the parser arms as the `Ty` head — `link.dart`'s
/// `getLinkingNode` returning `ast.AstNode?` into `simply_bounded.dart`,
/// which registers no `ast` prefix). The local `node` narrows on `is
/// ClassDeclaration`, but the armed head is the foreign projection scar
/// `ast?`, so the argument check must suppress rather than report the
/// census row's `argument_type_not_assignable 'ast?' can't be assigned
/// to 'AstNode'`.
#[test]
fn foreign_prefix_projection_head_is_silent() {
    let lib_source = "class AstNode {} \
                      class AnnotatedNode implements AstNode {} \
                      class Declaration implements AnnotatedNode {} \
                      class CompilationUnitMember implements Declaration {} \
                      class ClassDeclaration implements CompilationUnitMember {} \
                      ast.AstNode? getLinkingNode() => null;";
    // The diagnosing file imports `lib` both bare (the unprefixed face
    // `simply_bounded.dart` carries) and under `ast` (the same spelling
    // `link.dart`'s annotation used). `linker` is declared locally so the
    // call's return type arrives through the boundary with `ast` as its
    // armed head — the projection scar, not a type.
    let main_source = "import 'package:t/lib.dart'; \
                       import 'package:t/lib.dart' as ast; \
                       class Holder { final AstNode node; Holder(this.node); } \
                       void run() { \
                       var node = getLinkingNode(); \
                       if (node is ClassDeclaration) { Holder(node); } \
                       }";
    let rep = report_dual(main_source, lib_source, "ast");
    assert!(
        !has_code(&rep, "argument_type_not_assignable"),
        "{:?}",
        code_names(&rep)
    );
}
