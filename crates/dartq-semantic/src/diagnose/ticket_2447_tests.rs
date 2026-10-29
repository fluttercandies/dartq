//! The #2447 pins: a prefixed annotation arms its FULL qualified name —
//! `ast.AstNode?` projects the `Ty` head `ast.AstNode`, never the bare
//! prefix token the parser's `handle_type` completion carries.
//!
//! The parser's completion events were already truthful —
//! `PrefixedTypeReference` (the prefix), `TypeReferenceContinuation` (the
//! continuation), `handle_qualified` — but the collector armed the head
//! from the prefix token alone, dropping the continuation. A foreign
//! library's declared `ast.AstNode?` therefore crossed the boundary with
//! the head `ast`, the projection scar commit 87c65d96 gated downstream in
//! [`Engine::prefix_head_ty`]. The join at `handle_type` (the
//! `note_annotation_segment` dot-join idiom; upstream resolves
//! `prefix.Name` through the import namespace as one node — upstream Dart
//! SDK source, pkg/analyzer/lib/src/dart/ast/ast.dart) removes the scar at
//! its source: the gate goes inert for this shape and stays as defense in
//! depth for foreign boundary projections.
//!
//! Oracle provenance: /tmp/probe2447 (3.15 stays silent on the sb shape —
//! the dual-import `void run(ast.AstNode? node)` narrowed call — and on
//! the foreign projection).

use super::*;
use crate::collector::collect;
use crate::model::{ArgListId, Ty};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Scans and collects one unit — the element-level rig: the qualified
/// head lives on the declared element, observable without a library
/// graph (collection is import-agnostic).
fn collect_elements(source: &str) -> (FileElements, StringTable) {
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
    (elements, strings)
}

/// The named-head spelling of `ty`, panicking on any other shape.
fn named_head<'a>(ty: &Ty, strings: &'a StringTable) -> (&'a str, bool, ArgListId) {
    match ty {
        Ty::Named {
            name,
            nullable,
            args,
        } => (strings.resolve(*name), *nullable, *args),
        other => panic!("expected a named type, got {other:?}"),
    }
}

/// The local shape (the /tmp/probe2441/sb rig's own file): both
/// nullabilities of a written `ast.AstNode` annotation arm the joined
/// head — `ast.AstNode`, never the prefix token alone.
#[test]
fn prefixed_annotation_arms_the_qualified_head() {
    let source = "ast.AstNode? nullable_node; ast.AstNode plain_node;";
    let (elements, strings) = collect_elements(source);

    let (head, nullable, args) = named_head(
        &elements
            .decl(strings.intern("nullable_node"))
            .expect("declared")
            .var_ty,
        &strings,
    );
    assert_eq!(head, "ast.AstNode");
    assert!(nullable);
    assert_eq!(args, ArgListId::EMPTY);

    let (head, nullable, _) = named_head(
        &elements
            .decl(strings.intern("plain_node"))
            .expect("declared")
            .var_ty,
        &strings,
    );
    assert_eq!(head, "ast.AstNode");
    assert!(!nullable);
}

/// The projection face: a foreign top-level function declaring
/// `ast.AstNode?` as its return type (link.dart's `getLinkingNode` shape)
/// carries the joined head across the boundary — the `Ty` the boundary
/// projects into a diagnosing library's argument checks.
#[test]
fn foreign_projection_carries_the_qualified_head() {
    let source = "ast.AstNode? getLinkingNode() => null;";
    let (elements, strings) = collect_elements(source);
    let (head, nullable, _) = named_head(
        &elements
            .decl(strings.intern("getLinkingNode"))
            .expect("declared")
            .return_ty,
        &strings,
    );
    assert_eq!(head, "ast.AstNode");
    assert!(nullable);
}

/// A parameterized prefixed head joins too: `ast.Boxed<int>` arms
/// `ast.Boxed`, not `ast`.
#[test]
fn parameterized_prefixed_head_joins() {
    let source = "ast.Boxed<int> boxed;";
    let (elements, strings) = collect_elements(source);
    let (head, _, _) = named_head(
        &elements
            .decl(strings.intern("boxed"))
            .expect("declared")
            .var_ty,
        &strings,
    );
    assert_eq!(head, "ast.Boxed");
}

/// The join rides the argument-tree capture: a qualified type argument
/// (`Map<String, ast.AstNode>`) arms its own joined leaf inside the
/// outer head's argument list.
#[test]
fn qualified_argument_leaf_joins() {
    let source = "Map<String, ast.AstNode> m;";
    let (elements, strings) = collect_elements(source);
    let var_ty = &elements.decl(strings.intern("m")).expect("declared").var_ty;
    let (_, _, args) = named_head(var_ty, &strings);
    let pool = elements.type_args().get(args);
    assert_eq!(pool.len(), 2, "{var_ty:?}");
    let (head, _, _) = named_head(&pool[1], &strings);
    assert_eq!(head, "ast.AstNode");
}

/// Diagnoses `main_source` importing `lib_source` bare and under
/// `prefix` — the /tmp/probe2441/sb shape's rig (the dual import is what
/// lets one file write both the bare chain names and the `ast.` heads).
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
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
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

/// The sb verdict face (oracle /tmp/probe2447/sb: silent): the written
/// `ast.AstNode?` formal's argument check against the `is`-narrowed
/// argument stays silent with the projection scar gone — the joined head
/// reaches the relation undecided-or-compatible, never the prefix-token
/// fiction the pre-fix nullability matrix fabricated over.
#[test]
fn sb_prefixed_formal_argument_stays_silent() {
    let lib_source = "abstract class AstNode {} \
                      abstract class AnnotatedNode implements AstNode {} \
                      abstract class Declaration implements AnnotatedNode {} \
                      abstract class CompilationUnitMember implements Declaration {} \
                      abstract class ClassDeclaration implements CompilationUnitMember {} \
                      class Holder { final AstNode node; Holder(this.node); }";
    let main_source = "import 'lib.dart'; \
                       import 'lib.dart' as ast; \
                       void run(ast.AstNode? node) { \
                       if (node is ClassDeclaration) { Holder(node); } \
                       }";
    let rep = report_dual(main_source, lib_source, "ast");
    assert!(
        !rep.diagnostics
            .iter()
            .any(|d| d.code.name == "argument_type_not_assignable"),
        "{:?}",
        rep.diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}
