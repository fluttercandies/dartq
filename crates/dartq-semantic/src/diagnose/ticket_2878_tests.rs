//! The #2878 display-retention pins: a function type's own type-parameter
//! group (`void Function<T extends B>(...)`) and a named type's arguments
//! (`G<A<dynamic>>`) survive into the message slots, the official
//! `FunctionType` display form (`display_string_builder.dart`
//! `writeFunctionType` → `_writeTypeParameters`). The census faces: the
//! generic tear-off reads (`f` of `void f<T>(T a)` spells
//! `void Function<T>(T)`), the written-annotation shapes, and the
//! redirecting-factory faces (`LinkFactory<dynamic>` against `Link<T>`,
//! the instantiate-to-bounds reading of the bare reference).
//!
//! Oracle provenance: the #2872 census rows
//! (`FunctionReferenceResolutionTest/test_function_extensionOnFunction_static`,
//! `ConstWithTypeParametersTest/test_asExpression_functionType`,
//! `factory4_test`/`factory5_test`) against `dart analyze` (Dart 3.13,
//! fvm 3.47.1).

use super::*;
use crate::collector::collect;
use crate::model::ArgPool;
use crate::persisted::PersistedElements;
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

fn messages_of(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

// —— the model-level render ————————————————————————————————

/// The signature's own group renders verbatim: names always, bounds only
/// when written, the `?` suffix after the formal list — the official
/// display form byte-for-byte.
#[test]
fn render_spells_the_type_parameter_group() {
    let strings = StringTable::new();
    let name = |text: &str| strings.intern(text);
    let mut pool = ArgPool::default();
    let t = name("T");
    let a_never = pool.named(name("A"), false, std::slice::from_ref(&Ty::Never));
    let group: Box<[(StringId, Option<Ty>)]> = Box::new([(t, Some(a_never))]);
    let ty = pool.function_generic(&group, Ty::Void, &[], &[], false);
    assert_eq!(
        ty.render(&strings, &pool),
        "void Function<T extends A<Never>>()"
    );

    // An unbounded parameter spells its name alone.
    let group: Box<[(StringId, Option<Ty>)]> = Box::new([(t, None)]);
    let ty = pool.function_generic(
        &group,
        Ty::Void,
        std::slice::from_ref(&Ty::named(t)),
        &[],
        false,
    );
    assert_eq!(ty.render(&strings, &pool), "void Function<T>(T)");

    // The `?` suffix keeps its position after the formals.
    let ty = pool.function_generic(&group, Ty::Void, &[], &[], true);
    assert_eq!(ty.render(&strings, &pool), "void Function<T>()?");
}

/// A recovered invalid bound renders under the oracle's `InvalidType`
/// name (`writeInvalidType`), the same splice spelling a record hole
/// takes.
#[test]
fn render_spells_an_invalid_bound() {
    let strings = StringTable::new();
    let mut pool = ArgPool::default();
    let group: Box<[(StringId, Option<Ty>)]> = Box::new([(strings.intern("X"), Some(Ty::Unknown))]);
    let ty = pool.function_generic(&group, Ty::Void, &[], &[], false);
    assert_eq!(
        ty.render(&strings, &pool),
        "void Function<X extends InvalidType>()"
    );
}

/// The count-only producer keeps its byte-identical bare rendering —
/// every pre-#2878 shape displays exactly as before.
#[test]
fn count_only_shapes_render_byte_identically() {
    let strings = StringTable::new();
    let name = |text: &str| strings.intern(text);
    let mut pool = ArgPool::default();
    let int = Ty::named(name("int"));
    let ty = pool.function(1, Ty::Void, std::slice::from_ref(&int), &[], false);
    assert_eq!(ty.render(&strings, &pool), "void Function(int)");
    let ty = pool.function(0, Ty::Void, &[], &[], false);
    assert_eq!(ty.render(&strings, &pool), "void Function()");
}

/// Named-type arguments keep their written face (`G<A<dynamic>>`,
/// `LinkFactory<T>`) — the redirect face's display column.
#[test]
fn named_arguments_render_verbatim() {
    let strings = StringTable::new();
    let name = |text: &str| strings.intern(text);
    let mut pool = ArgPool::default();
    let dynamic_arg = pool.named(name("A"), false, std::slice::from_ref(&Ty::Dynamic));
    let g = pool.named(name("G"), false, std::slice::from_ref(&dynamic_arg));
    assert_eq!(g.render(&strings, &pool), "G<A<dynamic>>");
    let t = name("T");
    let factory = pool.named(
        name("LinkFactory"),
        false,
        std::slice::from_ref(&Ty::named(t)),
    );
    assert_eq!(factory.render(&strings, &pool), "LinkFactory<T>");
}

// —— the pool's identity ————————————————————————————————

/// Equal written shapes share one slot (the canonical-interning
/// contract); alpha-different spellings are distinct slots (a render
/// never borrows another annotation's names); the count-only and
/// with-names producers never merge.
#[test]
fn shape_identity_splits_on_the_group() {
    let strings = StringTable::new();
    let name = |text: &str| strings.intern(text);
    let mut pool = ArgPool::default();
    let t = name("T");
    let u = name("U");
    let group_t: Box<[(StringId, Option<Ty>)]> = Box::new([(t, None)]);
    let group_u: Box<[(StringId, Option<Ty>)]> = Box::new([(u, None)]);
    let a = pool.function_generic(&group_t, Ty::Void, &[], &[], false);
    let b = pool.function_generic(&group_t, Ty::Void, &[], &[], false);
    assert_eq!(a, b, "equal shapes share one slot");
    let c = pool.function_generic(&group_u, Ty::Void, &[], &[], false);
    assert_ne!(a, c, "alpha-different spellings are distinct slots");
    let d = pool.function(1, Ty::Void, &[], &[], false);
    assert_ne!(a, d, "count-only and with-names forms never merge");
}

/// The persisted column round-trips: a payload written with the group
/// restores it byte-exact; the pre-column decode default (`#[serde(default)]`)
/// keeps the count-only identity.
#[test]
fn payload_round_trips_the_group() {
    let strings = StringTable::new();
    let name = |text: &str| strings.intern(text);
    let mut elements = FileElements::default();
    let pool = elements.type_args_mut();
    let t = name("T");
    let group: Box<[(StringId, Option<Ty>)]> = Box::new([(t, Some(Ty::named(name("num"))))]);
    let generic = pool.function_generic(&group, Ty::Void, &[], &[], false);
    let counted = pool.function(1, Ty::Void, &[], &[], false);
    drop(pool);
    let elements = FileElements {
        decls: crate::model::NameMap::from_iter([
            (
                name("generic"),
                crate::model::TopDecl {
                    var_ty: generic,
                    ..crate::model::TopDecl::bare(MicroKind::TopLevelVar)
                },
            ),
            (
                name("counted"),
                crate::model::TopDecl {
                    var_ty: counted,
                    ..crate::model::TopDecl::bare(MicroKind::TopLevelVar)
                },
            ),
        ]),
        ..elements
    };

    let payload = PersistedElements::of(&elements, &strings);
    let restored = payload.into_elements(&strings);
    let pool = restored.type_args();
    let generic_back = restored.decl(name("generic")).expect("decl").var_ty;
    let counted_back = restored.decl(name("counted")).expect("decl").var_ty;
    assert_eq!(
        generic_back.render(&strings, pool),
        "void Function<T extends num>()",
        "the written group survives the payload"
    );
    assert_eq!(
        counted_back.render(&strings, pool),
        "void Function()",
        "the count-only shape keeps its bare face"
    );
}

// —— the message-slot faces ————————————————————————————————
//
// The end-to-end pins cover the faces that fire today (the redirect's
// instantiated forms, the annotation pair's spelled groups). The census
// receiver/element faces this substrate also feeds
// (`undefined_getter` over a function-typed receiver, the
// `const [f as ...]` list element) stay silent end-to-end — their
// report faces predate this ticket and never fire on those shapes
// (census missing-only rows); they light up with the correct text once
// their own face tickets land.

/// The redirecting factory's message spells the instantiated faces: the
/// bare target reads instantiate-to-bounds (`LinkFactory<dynamic>` under
/// an unbounded `LinkFactory<T>`), the enclosing class carries its own
/// parameters (`Link<T>`) — the oracle's factory4 corpus shape.
#[test]
fn redirect_spells_the_instantiated_faces() {
    let source = "abstract class Link<T> {\n\
                  \x20 factory Link.create() = LinkFactory.create;\n\
                  }\n\
                  \n\
                  class A<T> {}\n\
                  \n\
                  class LinkFactory<T> extends A<T> {\n\
                  \x20 factory LinkFactory.create() {\n\
                  \x20   return LinkFactory._();\n\
                  \x20 }\n\
                  \n\
                  \x20 LinkFactory._();\n\
                  }\n\
                  \n\
                  void main() {\n\
                  \x20 new Link<int>.create();\n\
                  }\n";
    let report = report(source);
    let messages = messages_of(&report, "redirect_to_invalid_return_type");
    assert!(
        messages.iter().any(|m| m
            == "The return type 'LinkFactory<dynamic>' of the redirected constructor isn't a subtype of 'Link<T>'."),
        "redirect diagnostics: {:?}", report.diagnostics
    );
}

/// The written-annotation pair spells both signatures' own groups (the
/// #2849 stashed-signature posture — the pre-#2878 display that already
/// carried the names; regression guard).
#[test]
fn annotation_pair_spells_both_groups() {
    let source = "void main() {\n\
                  \x20 String Function<T>(String) from;\n\
                  \x20 String Function<S>(int) to = from;\n\
                  }\n";
    let report = report(source);
    let messages = messages_of(&report, "invalid_assignment");
    assert!(
        messages.iter().any(|m| m
            == "A value of type 'String Function<T>(String)' can't be assigned to a variable of type 'String Function<S>(int)'."),
        "invalid_assignment messages: {messages:?}"
    );
}
